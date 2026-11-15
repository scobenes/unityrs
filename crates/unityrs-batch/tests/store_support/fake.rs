//! A test-only loopback S3 peer with independently checked signatures.
#![allow(dead_code)]
use super::{
    fake_ops::{self, Answer},
    sign,
};
use http_body_util::BodyExt;
use hyper::{
    Request, Response,
    body::{Body, Bytes, Frame, Incoming, SizeHint},
    service::service_fn,
};
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::AsyncRead;
use tokio::{
    net::TcpListener,
    task::{AbortHandle, JoinHandle},
    time::Sleep,
};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Put,
    Get,
    Head,
    List,
    Create,
    Part,
    Complete,
    Abort,
}

/// Composable, per-operation/per-key/per-try faults. Body delays occur after
/// the head or the selected body offset; read faults act on request DATA.
#[derive(Clone, Default)]
pub struct Fault {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub body: Option<Vec<u8>>,
    pub headers: Vec<(String, String)>,
    pub before_head: Option<Duration>,
    pub after_head: Option<Duration>,
    pub mid_body: Option<(usize, Duration)>,
    pub read_delay_per_kib: Option<Duration>,
    pub stop_reading_after: Option<usize>,
    pub close: bool,
    pub lost_response: bool,
    pub commit_delay: Option<Duration>,
    pub etag: Option<String>,
    pub upload_id: Option<String>,
}
impl Fault {
    pub fn error(status: u16, code: &str) -> Self {
        Self {
            status: Some(status),
            code: Some(code.into()),
            ..Self::default()
        }
    }
    pub fn redirect(status: u16, location: &str) -> Self {
        Self {
            status: Some(status),
            headers: vec![("location".into(), location.into())],
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct Log {
    pub operation: Operation,
    pub method: String,
    pub raw_path: String,
    pub raw_query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body_len: usize,
    pub connection: u64,
    pub accepted: Instant,
    pub reset: bool,
    pub try_ordinal: u32,
    pub signature_valid: bool,
}
impl Log {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log")
            .field("operation", &self.operation)
            .field("method_bytes", &self.method.len())
            .field("path_bytes", &self.raw_path.len())
            .field("query_bytes", &self.raw_query.as_ref().map(String::len))
            .field("headers", &self.headers.len())
            .field("body_len", &self.body_len)
            .field("connection", &self.connection)
            .field("accepted", &self.accepted)
            .field("reset", &self.reset)
            .field("try_ordinal", &self.try_ordinal)
            .field("signature_valid", &self.signature_valid)
            .finish()
    }
}
#[derive(Clone)]
pub struct Object {
    pub bytes: Vec<u8>,
    pub etag: String,
    pub content_type: Option<String>,
}
impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Object")
            .field("body_bytes", &self.bytes.len())
            .field("etag_bytes", &self.etag.len())
            .field(
                "content_type_bytes",
                &self.content_type.as_ref().map(String::len),
            )
            .finish()
    }
}
pub struct Upload {
    pub bucket: String,
    pub key: String,
    pub parts: BTreeMap<u32, Vec<u8>>,
    pub content_type: Option<String>,
}
impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload")
            .field("bucket_bytes", &self.bucket.len())
            .field("key_bytes", &self.key.len())
            .field("parts", &self.parts.len())
            .field(
                "content_type_bytes",
                &self.content_type.as_ref().map(String::len),
            )
            .finish()
    }
}
pub struct State {
    pub bucket: String,
    pub objects: BTreeMap<(String, String), Object>,
    pub uploads: BTreeMap<String, Upload>,
    pub upload_sequence: u64,
    credentials: sign::Credentials,
    path_style: bool,
    faults: BTreeMap<(Operation, String, u32), Fault>,
    ordinals: BTreeMap<(Operation, String), u32>,
    logs: Vec<Log>,
    children: Vec<AbortHandle>,
}

pub struct Config {
    pub bucket: String,
    pub path_style: bool,
    pub credentials: sign::Credentials,
    pub tls: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bucket: "bucket".into(),
            path_style: true,
            credentials: sign::Credentials::default(),
            tls: false,
        }
    }
}

/// Start inside the test's current-thread runtime. All accept/connection
/// tasks are test infrastructure, aborted when this owner is dropped.
pub struct Fake {
    pub endpoint: Url,
    state: Arc<Mutex<State>>,
    listener: JoinHandle<()>,
}
impl Fake {
    pub async fn start() -> Self {
        Self::start_with(Config::default()).await
    }
    pub async fn start_with(config: Config) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let tls = config
            .tls
            .then(|| tokio_rustls::TlsAcceptor::from(server_tls()));
        let state = Arc::new(Mutex::new(State {
            bucket: config.bucket,
            objects: BTreeMap::new(),
            uploads: BTreeMap::new(),
            upload_sequence: 0,
            credentials: config.credentials,
            path_style: config.path_style,
            faults: BTreeMap::new(),
            ordinals: BTreeMap::new(),
            logs: Vec::new(),
            children: Vec::new(),
        }));
        let shared = state.clone();
        let listener = tokio::spawn(async move {
            let mut connection_sequence = 0;
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                connection_sequence += 1;
                let ordinal = connection_sequence;
                let accepted = Instant::now();
                let state = shared.clone();
                let tls = tls.clone();
                let task = tokio::spawn(async move {
                    if let Some(tls) = tls {
                        if let Ok(stream) = tls.accept(stream).await {
                            connection(state, ordinal, accepted, stream).await;
                        }
                    } else {
                        connection(state, ordinal, accepted, stream).await;
                    }
                });
                shared.lock().unwrap().children.push(task.abort_handle());
            }
        });
        Self {
            endpoint: Url::parse(&if config.tls {
                format!("https://localhost:{}", address.port())
            } else {
                format!("http://{address}")
            })
            .unwrap(),
            state,
            listener,
        }
    }
    pub fn schedule(&self, operation: Operation, key: &str, try_ordinal: u32, fault: Fault) {
        self.state
            .lock()
            .unwrap()
            .faults
            .insert((operation, key.into(), try_ordinal), fault);
    }
    pub fn logs(&self) -> Vec<Log> {
        self.state.lock().unwrap().logs.clone()
    }
    pub fn object(&self, bucket: &str, key: &str) -> Option<Object> {
        self.state
            .lock()
            .unwrap()
            .objects
            .get(&(bucket.into(), key.into()))
            .cloned()
    }
    pub fn seed(&self, bucket: &str, key: &str, bytes: &[u8]) {
        self.state.lock().unwrap().objects.insert(
            (bucket.into(), key.into()),
            Object {
                bytes: bytes.into(),
                etag: fake_ops::digest(bytes),
                content_type: None,
            },
        );
    }
    pub fn open_uploads(&self) -> usize {
        self.state.lock().unwrap().uploads.len()
    }
    pub fn address(&self) -> std::net::SocketAddr {
        (std::net::Ipv4Addr::LOCALHOST, self.endpoint.port().unwrap()).into()
    }
}

pub const ROOT: &[u8] = include_bytes!("../http_support/root.der");
fn server_tls() -> Arc<rustls::ServerConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(
                    include_bytes!("../http_support/leaf.der").to_vec(),
                )],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    include_bytes!("../http_support/key.der").to_vec(),
                )),
            )
            .unwrap(),
    )
}

async fn connection<I>(state: Arc<Mutex<State>>, ordinal: u64, accepted: Instant, stream: I)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let handler_state = state.clone();
    let service =
        service_fn(move |request| handle(handler_state.clone(), ordinal, accepted, request));
    let stream = Observe {
        inner: stream,
        state: state.clone(),
        ordinal,
    };
    let result = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .without_shutdown()
        .await;
    let reset = match result {
        Err(error) => is_reset(&error),
        Ok(parts) => {
            // A Connection: close response ends hyper before the peer's close.
            // Retain the transport to observe the peer's RST rather than closing
            // our side first and losing that evidence.
            let mut stream = parts.io.into_inner();
            let mut byte = [0];
            let read = std::future::poll_fn(|cx| {
                Pin::new(&mut stream).poll_read(cx, &mut tokio::io::ReadBuf::new(&mut byte))
            });
            matches!(tokio::time::timeout(Duration::from_secs(1),read).await,Ok(Err(error)) if error.kind()==io::ErrorKind::ConnectionReset)
        }
    };
    if reset {
        for log in &mut state.lock().unwrap().logs {
            if log.connection == ordinal {
                log.reset = true;
            }
        }
    }
}

// Hyper can consume a peer reset while ending an otherwise completed request.
// Record the actual IO result before its HTTP state machine classifies it.
struct Observe<I> {
    inner: I,
    state: Arc<Mutex<State>>,
    ordinal: u64,
}
impl<I> Observe<I> {
    fn record<T>(&self, result: &Poll<io::Result<T>>) {
        if matches!(result,Poll::Ready(Err(error)) if error.kind()==io::ErrorKind::ConnectionReset)
        {
            for log in &mut self.state.lock().unwrap().logs {
                if log.connection == self.ordinal {
                    log.reset = true;
                }
            }
        }
    }
}
impl<I: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Observe<I> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        self.record(&result);
        result
    }
}
impl<I: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Observe<I> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buffer);
        self.record(&result);
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(feature = "test-hooks")]
pub struct Resolver {
    pub address: std::net::SocketAddr,
    pub delay: Duration,
    pub names: Mutex<Vec<String>>,
}
#[cfg(feature = "test-hooks")]
impl Resolver {
    pub fn new(fake: &Fake) -> Self {
        Self {
            address: fake.address(),
            delay: Duration::ZERO,
            names: Mutex::new(Vec::new()),
        }
    }
}
#[cfg(feature = "test-hooks")]
impl unityrs_batch::http::resolve::Resolver for Resolver {
    fn resolve(
        &self,
        host: &str,
        _port: u16,
    ) -> io::Result<unityrs_batch::http::resolve::Addresses> {
        self.names.lock().unwrap().push(host.into());
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        Ok(unityrs_batch::http::resolve::Addresses::first([
            self.address
        ]))
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.listener.abort();
        for task in &self.state.lock().unwrap().children {
            task.abort();
        }
    }
}

fn is_reset(error: &hyper::Error) -> bool {
    use std::error::Error;
    let mut source = error.source();
    while let Some(error) = source {
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::ConnectionReset)
        {
            return true;
        }
        source = error.source();
    }
    false
}

async fn handle(
    state: Arc<Mutex<State>>,
    connection: u64,
    accepted: Instant,
    request: Request<Incoming>,
) -> io::Result<Response<FaultBody>> {
    let (head, mut body) = request.into_parts();
    let path_style = state.lock().unwrap().path_style;
    let Some(mut input) =
        fake_ops::identify(&head.method, &head.uri, head.headers.clone(), path_style)
    else {
        return response(Answer::error(400, "InvalidRequest"), &Fault::default());
    };
    let (fault, index) = begin(
        &mut state.lock().unwrap(),
        &input,
        &head,
        connection,
        accepted,
    );
    read_body(&state, index, &mut body, &mut input, &fault).await?;
    let signature_valid = {
        let state = state.lock().unwrap();
        sign::verify(
            &head.method,
            &head.uri,
            &head.headers,
            &input.body,
            &state.credentials,
        )
    };
    state.lock().unwrap().logs[index].signature_valid = signature_valid;
    if !signature_valid {
        return response(
            Answer::error(403, "SignatureDoesNotMatch"),
            &Fault::default(),
        );
    }
    if let Some(delay) = fault.before_head {
        tokio::time::sleep(delay).await;
    }
    if fault.close {
        return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
    }
    let mut answer = if let Some(delay) = fault.commit_delay {
        if input.operation != Operation::Put {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let mut answer = Answer::bytes(200, Vec::new());
        answer
            .headers
            .push(("etag".into(), fake_ops::digest(&input.body)));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            fake_ops::apply(&mut shared.lock().unwrap(), &input);
        });
        state.lock().unwrap().children.push(task.abort_handle());
        answer
    } else if fault.status.is_some() || fault.code.is_some() || fault.body.is_some() {
        if let Some(code) = &fault.code {
            Answer::error(fault.status.unwrap_or(200), code)
        } else {
            Answer::bytes(
                fault.status.unwrap_or(200),
                fault.body.clone().unwrap_or_default(),
            )
        }
    } else {
        ordinary_answer(&mut state.lock().unwrap(), &input, &fault)
    };
    if let Some(status) = fault.status {
        answer.status = status;
    }
    if let Some(body) = &fault.body {
        answer.body.clone_from(body);
    }
    if fault.lost_response {
        return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
    }
    answer.headers.extend(fault.headers.clone());
    response(answer, &fault)
}

fn begin(
    state: &mut State,
    input: &fake_ops::Input,
    head: &hyper::http::request::Parts,
    connection: u64,
    accepted: Instant,
) -> (Fault, usize) {
    let ordinal = state
        .ordinals
        .entry((input.operation, input.key.clone()))
        .or_default();
    *ordinal += 1;
    let ordinal = *ordinal;
    let fault = state
        .faults
        .remove(&(input.operation, input.key.clone(), ordinal))
        .unwrap_or_default();
    let index = state.logs.len();
    state.logs.push(Log {
        operation: input.operation,
        method: head.method.to_string(),
        raw_path: head.uri.path().into(),
        raw_query: head.uri.query().map(Into::into),
        headers: head
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    if matches!(name.as_str(), "authorization" | "x-amz-security-token") {
                        "<redacted>".into()
                    } else {
                        String::from_utf8_lossy(value.as_bytes()).into_owned()
                    },
                )
            })
            .collect(),
        body_len: 0,
        connection,
        accepted,
        reset: false,
        try_ordinal: ordinal,
        signature_valid: false,
    });
    (fault, index)
}

async fn read_body(
    state: &Mutex<State>,
    index: usize,
    body: &mut Incoming,
    input: &mut fake_ops::Input,
    fault: &Fault,
) -> io::Result<()> {
    if fault.stop_reading_after == Some(0) {
        return std::future::pending().await;
    }
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(io::Error::other)?;
        if let Ok(bytes) = frame.into_data() {
            for chunk in bytes.chunks(1024) {
                let count = fault.stop_reading_after.map_or(chunk.len(), |limit| {
                    chunk.len().min(limit.saturating_sub(input.body.len()))
                });
                input.body.extend_from_slice(&chunk[..count]);
                state.lock().unwrap().logs[index].body_len = input.body.len();
                if fault
                    .stop_reading_after
                    .is_some_and(|limit| input.body.len() >= limit)
                {
                    return std::future::pending().await;
                }
                if let Some(delay) = fault.read_delay_per_kib {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Ok(())
}

fn ordinary_answer(state: &mut State, input: &fake_ops::Input, fault: &Fault) -> Answer {
    let mut answer = fake_ops::apply(state, input);
    if let Some(id) = &fault.upload_id {
        let old = format!("upload-{}", state.upload_sequence);
        if input.operation == Operation::Create {
            if let Some(upload) = state.uploads.remove(&old) {
                state.uploads.insert(id.clone(), upload);
            }
            replace_xml_field(&mut answer.body, "UploadId", id);
        }
    }
    if let Some(etag) = &fault.etag {
        if let Some(object) = state
            .objects
            .get_mut(&(input.bucket.clone(), input.key.clone()))
        {
            object.etag.clone_from(etag);
        }
        answer.headers.retain(|(name, _)| name != "etag");
        answer.headers.push(("etag".into(), etag.clone()));
        if input.operation == Operation::Complete {
            replace_xml_field(&mut answer.body, "ETag", etag);
        }
    }
    answer
}

fn replace_xml_field(body: &mut Vec<u8>, field: &str, value: &str) {
    let text = String::from_utf8(std::mem::take(body)).unwrap();
    let open = format!("<{field}>");
    let close = format!("</{field}>");
    if let Some((before, rest)) = text.split_once(&open)
        && let Some((_, after)) = rest.split_once(&close)
    {
        *body = format!("{before}{open}{}{close}{after}", fake_ops::xml(value)).into_bytes();
    } else {
        *body = text.into_bytes();
    }
}

fn response(answer: Answer, fault: &Fault) -> io::Result<Response<FaultBody>> {
    let mut response = Response::builder().status(answer.status);
    for (name, value) in answer.headers {
        response = response.header(name, value);
    }
    let length = answer.body.len();
    let body = FaultBody {
        bytes: Some(Bytes::from(answer.body)),
        after: fault.after_head,
        mid: fault.mid_body,
        sleep: None,
        split: false,
    };
    let mut response = response.body(body).map_err(io::Error::other)?;
    if !response.headers().contains_key("content-length") {
        response
            .headers_mut()
            .insert("content-length", length.to_string().parse().unwrap());
    }
    Ok(response)
}

pub struct FaultBody {
    bytes: Option<Bytes>,
    after: Option<Duration>,
    mid: Option<(usize, Duration)>,
    sleep: Option<Pin<Box<Sleep>>>,
    split: bool,
}
impl Body for FaultBody {
    type Data = Bytes;
    type Error = io::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        if let Some(delay) = self.after.take() {
            self.sleep = Some(Box::pin(tokio::time::sleep(delay)));
        }
        if let Some(sleep) = &mut self.sleep {
            if sleep.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.sleep = None;
        }
        let Some(mut bytes) = self.bytes.take() else {
            return Poll::Ready(None);
        };
        if !self.split
            && let Some((offset, delay)) = self.mid.take()
        {
            let count = offset.min(bytes.len());
            let first = bytes.split_to(count);
            self.bytes = Some(bytes);
            self.sleep = Some(Box::pin(tokio::time::sleep(delay)));
            self.split = true;
            return Poll::Ready(Some(Ok(Frame::data(first))));
        }
        Poll::Ready(Some(Ok(Frame::data(bytes))))
    }
    fn is_end_stream(&self) -> bool {
        self.bytes.is_none()
    }
    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(self.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64));
        hint
    }
}
