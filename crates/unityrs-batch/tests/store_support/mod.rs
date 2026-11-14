//! An in-memory store transport: each request takes the
//! next scripted reply, whose bytes arrive one chunk per read once the
//! request has been written. No socket, no resolver, no network.
#![allow(dead_code)]
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use unityrs_batch::{
    http::{self, Detail, Failure, Payload, Response, permit::PermitPool, permit::RequestPermit},
    store::{
        Kind,
        attempt::{Done, Fault, Policy, Tries, TrySlot},
        classify::Ended,
        client::{self, Settings},
        connector::{Connector, Transport},
    },
};
use unityrs_testkit::guard::DEADLOCK_GUARD;
use url::Url;

pub fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(DEADLOCK_GUARD, future)
                .await
                .expect("deadlock guard")
        })
}

/// How a reply's bytes end once its chunks are read.
#[derive(Clone, Copy, Debug)]
pub enum End {
    Eof,
    Stall,
    Reset,
}

pub enum Reply {
    /// The connection is up; the peer answers with these chunks.
    Answer { chunks: Vec<Vec<u8>>, end: End },
    /// The connection never comes up.
    Refuse(Detail),
    /// The connect never ends.
    StallConnect,
}

impl Reply {
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Answer {
            chunks: vec![bytes.into()],
            end: End::Eof,
        }
    }
    pub fn chunks(chunks: Vec<Vec<u8>>, end: End) -> Self {
        Self::Answer { chunks, end }
    }
    /// A response with this status, `Content-Length` and body.
    pub fn status(status: u16, headers: &str, body: &[u8]) -> Self {
        let mut bytes = format!(
            "HTTP/1.1 {status} X\r\n{headers}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        Self::bytes(bytes)
    }
    pub fn error(status: u16, code: &str) -> Self {
        Self::status(
            status,
            "",
            format!(
                "<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>m</Message></Error>"
            )
            .as_bytes(),
        )
    }
}

/// What the peer saw: each request's bytes as written.
#[derive(Default)]
pub struct Seen {
    pub requests: Mutex<Vec<Arc<Mutex<Vec<u8>>>>>,
    /// Bytes the peer served after each request's head.
    pub served: AtomicU64,
}

impl Seen {
    pub fn request(&self, index: usize) -> String {
        let requests = self.requests.lock().unwrap();
        String::from_utf8_lossy(&requests[index].lock().unwrap()).into_owned()
    }
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    /// A request's head lines, after its request line, as `name: value`.
    pub fn header(&self, index: usize, name: &str) -> Option<String> {
        let text = self.request(index);
        let head = text.split("\r\n\r\n").next().unwrap_or_default().to_owned();
        head.lines().skip(1).find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    }
    pub fn line(&self, index: usize) -> String {
        self.request(index)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

#[derive(Clone, Default)]
pub struct Script {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    pub seen: Arc<Seen>,
    pub sent: Arc<AtomicUsize>,
}

impl Script {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into_iter().collect())),
            ..Self::default()
        }
    }
    pub fn push(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }
}

impl Transport for Script {
    type Io = ScriptIo;
    async fn send(
        &self,
        permit: &mut RequestPermit,
        _url: &Url,
        request: hyper::Request<Payload>,
        _timeout: Duration,
        connected: &AtomicBool,
    ) -> Result<Response<ScriptIo>, http::Error> {
        self.sent.fetch_add(1, Ordering::AcqRel);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted reply");
        match reply {
            Reply::Refuse(detail) => Err(http::Error::Transport(Failure {
                detail,
                errno: None,
            })),
            Reply::StallConnect => std::future::pending().await,
            Reply::Answer { chunks, end } => {
                connected.store(true, Ordering::Release);
                let writes = Arc::new(Mutex::new(Vec::new()));
                self.seen.requests.lock().unwrap().push(writes.clone());
                let io = ScriptIo {
                    chunks: chunks.into(),
                    offset: 0,
                    end,
                    writes,
                    reader: None,
                    seen: self.seen.clone(),
                };
                http::exchange(io, permit, request).await
            }
        }
    }
}

pub struct ScriptIo {
    chunks: VecDeque<Vec<u8>>,
    offset: usize,
    end: End,
    writes: Arc<Mutex<Vec<u8>>>,
    reader: Option<Waker>,
    seen: Arc<Seen>,
}

impl AsyncRead for ScriptIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // The peer answers once the request's head has arrived.
        if !self
            .writes
            .lock()
            .unwrap()
            .windows(4)
            .any(|w| w == b"\r\n\r\n")
        {
            self.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let this = &mut *self;
        let Some(chunk) = this.chunks.front() else {
            return match this.end {
                End::Eof => Poll::Ready(Ok(())),
                End::Stall => Poll::Pending,
                End::Reset => Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
            };
        };
        let count = (chunk.len() - this.offset).min(buf.remaining());
        buf.put_slice(&chunk[this.offset..this.offset + count]);
        this.offset += count;
        this.seen.served.fetch_add(count as u64, Ordering::AcqRel);
        if this.offset == chunk.len() {
            this.chunks.pop_front();
            this.offset = 0;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ScriptIo {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.writes.lock().unwrap().extend_from_slice(buf);
        if let Some(reader) = this.reader.take() {
            reader.wake();
        }
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub const SECOND: Duration = Duration::from_secs(1);

pub fn policy(attempts: u32) -> Policy {
    Policy {
        attempts,
        timeout: Duration::from_secs(30),
        backoff_initial_ms: 0,
        backoff_max_ms: 0,
        cutoff: None,
    }
}

/// A store over a scripted transport, its slot's permit from a pool of one.
pub struct Harness {
    pub script: Script,
    pub store: object_store::aws::AmazonS3,
    pub slot: Arc<TrySlot>,
    pub pool: PermitPool,
    pub stop: AtomicBool,
    pub unsettled: AtomicU64,
    pub policy: Policy,
}

pub fn endpoint() -> Url {
    Url::parse("https://s3.example.test:9000").unwrap()
}

impl Harness {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        let url = endpoint();
        Self::with(
            &Settings {
                bucket: "bucket",
                region: "us-east-1",
                endpoint: Some(&url),
                path_style: true,
            },
            None,
            replies,
        )
    }
    pub fn with(
        settings: &Settings<'_>,
        content_type: Option<&str>,
        replies: impl IntoIterator<Item = Reply>,
    ) -> Self {
        let script = Script::new(replies);
        let connector = Connector::new(
            script.clone(),
            client::origin(settings).unwrap(),
            Duration::from_secs(10),
        );
        let credentials = credentials("AKIDEXAMPLE", "secret", None);
        let store = client::build(settings, content_type, &credentials, &connector).unwrap();
        let pool = PermitPool::new(1).unwrap();
        let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
        Self {
            script,
            store,
            slot,
            pool,
            stop: AtomicBool::new(false),
            unsettled: AtomicU64::new(0),
            policy: policy(1),
        }
    }
    pub fn tries(&self) -> Tries<'_> {
        Tries {
            policy: self.policy,
            control: &self.stop,
            unsettled: &self.unsettled,
        }
    }
    /// Runs `op`'s tries of `kind`, each error an `Ended::Error`.
    pub fn run<T, F, Fut>(&self, kind: Kind, op: F) -> Result<Done<T>, Fault>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = object_store::Result<T>>,
    {
        let mut op = op;
        run(self.tries().run(&self.slot, kind, |n| {
            let call = op(n);
            async move { call.await.map_err(|_| Ended::Error) }
        }))
    }
    pub fn unsettled(&self) -> u64 {
        self.unsettled.load(Ordering::Acquire)
    }
}

pub fn credentials(
    key: &str,
    secret: &str,
    token: Option<&str>,
) -> object_store::aws::AwsCredentialProvider {
    let env = |name: &str| match name {
        "AWS_ACCESS_KEY_ID" => Some(key.into()),
        "AWS_SECRET_ACCESS_KEY" => Some(secret.into()),
        "AWS_SESSION_TOKEN" => token.map(Into::into),
        _ => None,
    };
    unityrs_batch::store::credentials::read(env, None)
        .unwrap()
        .into_provider()
}
