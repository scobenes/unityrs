//! `object_store`'s `HttpConnector` over the engine's HTTP service
//! (`docs/plans/batch.md` 2.9). Each `call` is its try's one request: it
//! takes the task-local [`TrySlot`]'s permit, turns `object_store`'s request
//! into the service's origin-form request, sends it, records the try's
//! status, `Retry-After` and `<Code>`, and hands back a body the connector
//! bounds. Every error it returns is opaque: no URL, key, header or
//! third-party text; the table reads the slot instead.
use super::{
    Kind,
    attempt::{Fault, PermitGuard, TrySlot},
    body,
};
use crate::http::{self, Payload, Response, permit::RequestPermit};
use hyper::{
    body::{Body, Bytes},
    http::{
        HeaderMap, StatusCode, Uri,
        header::{ACCEPT, CONTENT_LENGTH, HOST, USER_AGENT},
    },
};
use object_store::{
    ClientOptions,
    client::{
        HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpRequestBody,
        HttpResponse, HttpService,
    },
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, atomic::AtomicBool},
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;

/// What sends one prepared request: the engine's [`http::Service`], or an
/// in-memory transport in tests. `connected` is set once
/// the connection is up.
pub trait Transport: Send + Sync + 'static {
    type Io: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn send<'a>(
        &'a self,
        permit: &'a mut RequestPermit,
        url: &'a Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        connected: &'a AtomicBool,
    ) -> impl Future<Output = Result<Response<Self::Io>, http::Error>> + Send + 'a;
}

impl Transport for http::Service {
    type Io = http::Stream;
    fn send<'a>(
        &'a self,
        permit: &'a mut RequestPermit,
        url: &'a Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        connected: &'a AtomicBool,
    ) -> impl Future<Output = Result<Response<Self::Io>, http::Error>> + Send + 'a {
        self.request_observed(permit, url, request, timeout, connected)
    }
}

/// The one scheme, host and port a sink's requests may go to: `https`
/// unless the spec's endpoint is `http` (E37), its bucket's host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
}

impl Origin {
    /// The origin of a client's bucket endpoint (`client::bucket_endpoint`).
    pub fn of(url: &Url) -> Result<Self, Fault> {
        let https = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(Fault::Internal),
        };
        let host = url.host_str().ok_or(Fault::Internal)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(host.len())
            .map_err(|_| Fault::Memory)?;
        owned.push_str(host);
        Ok(Self {
            https,
            host: owned,
            port: url.port_or_known_default().ok_or(Fault::Internal)?,
        })
    }
    fn admits(&self, url: &Url) -> bool {
        url.scheme() == if self.https { "https" } else { "http" }
            && url.host_str() == Some(self.host.as_str())
            && url.port_or_known_default() == Some(self.port)
    }
}

struct Shared<T> {
    transport: T,
    origin: Origin,
    connect_timeout: Duration,
}

/// One per runtime: the controller's clients share one, the finisher's has
/// its own. Cloning shares it.
pub struct Connector<T: Transport> {
    shared: Arc<Shared<T>>,
}

impl<T: Transport> Connector<T> {
    /// Startup: a sink's origin and `connect_timeout_ms` (1.4, E37).
    #[must_use]
    pub fn new(transport: T, origin: Origin, connect_timeout: Duration) -> Self {
        Self {
            shared: Arc::new(Shared {
                transport,
                origin,
                connect_timeout,
            }),
        }
    }
}
impl<T: Transport> Clone for Connector<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}
impl<T: Transport> fmt::Debug for Connector<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Connector")
    }
}

/// `object_store` applies its client options only in its own connectors,
/// so none reaches this one (`src/client/mod.rs`).
impl<T: Transport> HttpConnector for Connector<T> {
    fn connect(&self, _: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(Engine {
            shared: Arc::clone(&self.shared),
        }))
    }
}

struct Engine<T: Transport> {
    shared: Arc<Shared<T>>,
}
impl<T: Transport> fmt::Debug for Engine<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Engine")
    }
}

/// `async_trait`'s form of `HttpService::call`, written out.
impl<T: Transport> HttpService for Engine<T> {
    fn call<'life0, 'async_trait>(
        &'life0 self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.serve(request))
    }
}

/// The text of every error the connector returns.
#[derive(Debug)]
pub(super) struct Refused;
impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("store request failed")
    }
}
impl std::error::Error for Refused {}

/// An opaque error: its source is a unit, so it allocates nothing.
pub(super) fn refused() -> HttpError {
    HttpError::new(HttpErrorKind::Unknown, Refused)
}

enum Failure {
    Fault(Fault),
    Transport(http::Error),
}
impl From<Fault> for Failure {
    fn from(fault: Fault) -> Self {
        Self::Fault(fault)
    }
}

impl<T: Transport> Engine<T> {
    async fn serve(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let Some(slot) = TrySlot::current() else {
            return Err(refused());
        };
        let (kind, mut guard) = slot.enter().map_err(|_| refused())?;
        match self.exchange(&slot, kind, &mut guard, request).await {
            Ok(response) => Ok(response),
            Err(Failure::Fault(fault)) => {
                slot.fail(fault);
                Err(refused())
            }
            Err(Failure::Transport(error)) => {
                match error {
                    http::Error::Transport(failure) => {
                        slot.update(|r| r.detail = Some(failure.detail));
                    }
                    http::Error::Memory => slot.fail(Fault::Memory),
                    _ => slot.fail(Fault::Internal),
                }
                Err(refused())
            }
        }
    }

    async fn exchange(
        &self,
        slot: &Arc<TrySlot>,
        kind: Kind,
        guard: &mut PermitGuard,
        request: HttpRequest,
    ) -> Result<HttpResponse, Failure> {
        let (parts, body) = request.into_parts();
        let url = url_of(&parts.uri)?;
        if !self.shared.origin.admits(&url) {
            return Err(Fault::Internal.into());
        }
        let headers = headers(parts.headers, &url, body.content_length())?;
        let request = http::prepare(&url, parts.method, headers, payload(body)?).map_err(
            |error| match error {
                http::Error::Memory => Fault::Memory,
                _ => Fault::Internal,
            },
        )?;
        let permit = guard.permit().ok_or(Fault::Internal)?;
        let mut response = self
            .shared
            .transport
            .send(
                permit,
                &url,
                request,
                self.shared.connect_timeout,
                slot.connected_flag(),
            )
            .await
            .map_err(Failure::Transport)?;
        let status = response.status();
        let after = crate::stage::retry::retry_after(response.headers());
        slot.update(|r| {
            r.status = Some(status);
            r.retry_after = after;
        });
        let code = StatusCode::from_u16(status).map_err(|_| Fault::Internal)?;
        // The head's fields go to `object_store` as they came (`C_net`).
        let head = response.headers().clone();
        let body = if !(200..300).contains(&status) {
            let permit = guard.permit().ok_or(Fault::Internal)?;
            body::error_body(slot, permit, &mut response).await?
        } else if kind == Kind::ListProof {
            body::drain(slot, &mut response).await
        } else {
            body::capped(slot, kind, response)
        };
        let mut out = HttpResponse::new(body);
        *out.status_mut() = code;
        *out.headers_mut() = head;
        Ok(out)
    }
}

/// `object_store`'s `Uri` as the `url` crate reads it, which the service
/// sends: its origin-form path and query must stay the same bytes, or the
/// signature would not match what is sent.
fn url_of(uri: &Uri) -> Result<Url, Fault> {
    let (Some(scheme), Some(authority), Some(path)) =
        (uri.scheme_str(), uri.authority(), uri.path_and_query())
    else {
        return Err(Fault::Internal);
    };
    let mut text = String::new();
    text.try_reserve_exact(scheme.len() + 3 + authority.as_str().len() + path.as_str().len())
        .map_err(|_| Fault::Memory)?;
    text.push_str(scheme);
    text.push_str("://");
    text.push_str(authority.as_str());
    text.push_str(path.as_str());
    let url = Url::parse(&text).map_err(|_| Fault::Internal)?;
    if url[url::Position::BeforePath..url::Position::AfterQuery].as_bytes()
        != path.as_str().as_bytes()
    {
        return Err(Fault::Internal);
    }
    Ok(url)
}

/// `Host`, which the signer sets, must be the URL's authority, and
/// `Content-Length` the body's length; the service writes both itself. A
/// `User-Agent` or `Accept` of `object_store`'s would take the engine's
/// place, which store requests keep (1.4), so neither may be there.
fn headers(mut headers: HeaderMap, url: &Url, length: usize) -> Result<HeaderMap, Fault> {
    let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let mut hosts = headers.get_all(HOST).iter();
    if hosts
        .next()
        .is_some_and(|host| host.as_bytes() != authority.as_bytes())
        || hosts.next().is_some()
    {
        return Err(Fault::Internal);
    }
    let mut lengths = headers.get_all(CONTENT_LENGTH).iter();
    if lengths
        .next()
        .is_some_and(|value| decimal(value.as_bytes()) != Some(length as u64))
        || lengths.next().is_some()
        || headers.contains_key(USER_AGENT)
        || headers.contains_key(ACCEPT)
    {
        return Err(Fault::Internal);
    }
    headers.remove(HOST);
    headers.remove(CONTENT_LENGTH);
    Ok(headers)
}

/// A `Content-Length`'s digits; `None` for anything else.
pub(super) fn decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 19 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        bytes
            .iter()
            .fold(0_u64, |n, &b| n * 10 + u64::from(b - b'0')),
    )
}

/// The body as one `Bytes`, uncopied: the engine builds every payload as
/// one, and `object_store`'s own bodies are one string. More than one
/// chunk is the engine's fault.
fn payload(mut body: HttpRequestBody) -> Result<Payload, Fault> {
    if let Some(bytes) = body.as_bytes() {
        return Ok(Payload::new(bytes.clone()));
    }
    let mut cx = Context::from_waker(Waker::noop());
    let mut one: Option<Bytes> = None;
    loop {
        match Pin::new(&mut body).poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => {
                let Ok(data) = frame.into_data() else {
                    return Err(Fault::Internal);
                };
                if data.is_empty() {
                    continue;
                }
                if one.replace(data).is_some() {
                    return Err(Fault::Internal);
                }
            }
            Poll::Ready(None) => return Ok(Payload::new(one.unwrap_or_default())),
            Poll::Ready(Some(Err(_))) | Poll::Pending => return Err(Fault::Internal),
        }
    }
}

#[cfg(test)]
#[path = "connector_tests.rs"]
mod tests;
