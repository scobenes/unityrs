//! Exact-size payload frames and origin-form request preparation.
use super::Error;
use hyper::{
    Request,
    body::{Body, Bytes, Frame, SizeHint},
    http::{HeaderMap, HeaderValue, Method, Uri, header},
};
use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};
use url::Url;

pub const PIECE: usize = 64 * 1_024;
const MAX_URL: usize = 16 * 1_024;

/// The payload's backing storage is the caller's already charged phase buffer.
/// Slices retain that backing; their size is not a new allocation charge.
#[derive(Default)]
pub struct Payload {
    bytes: Bytes,
}
impl Payload {
    #[must_use]
    pub fn new(bytes: Bytes) -> Self {
        Self { bytes }
    }
}
impl Body for Payload {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if self.bytes.is_empty() {
            Poll::Ready(None)
        } else {
            let count = self.bytes.len().min(PIECE);
            Poll::Ready(Some(Ok(Frame::data(self.bytes.split_to(count)))))
        }
    }
    fn is_end_stream(&self) -> bool {
        self.bytes.is_empty()
    }
    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))
    }
}
/// Prepared headers and payload belong to `C_hdr` and the phase budget. The
/// active request's URI and encoded head belong to its network envelope.
/// No header or URL is included in a returned error's diagnostic.
pub fn prepare(
    url: &Url,
    method: Method,
    mut headers: HeaderMap,
    body: Payload,
) -> Result<Request<Payload>, Error> {
    validate_url(url)?;
    if headers.len() > 32
        || headers.contains_key(header::HOST)
        || headers.contains_key(header::CONTENT_LENGTH)
        || headers.contains_key(header::TRANSFER_ENCODING)
        || headers.contains_key(header::CONNECTION)
        || headers.contains_key(header::RANGE)
        || headers.contains_key(header::ACCEPT_ENCODING)
    {
        return Err(Error::InvalidRequest);
    }
    headers.try_reserve(4).map_err(|_| Error::InvalidRequest)?;
    let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let host = owned_value(authority.as_bytes())?;
    headers.insert(header::HOST, host);
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    headers
        .entry(header::USER_AGENT)
        .or_insert(HeaderValue::from_static(concat!(
            "unityrs/",
            env!("CARGO_PKG_VERSION")
        )));
    headers
        .entry(header::ACCEPT)
        .or_insert(HeaderValue::from_static("*/*"));
    let origin = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(origin.len())
        .map_err(|_| Error::Memory)?;
    bytes.extend_from_slice(origin.as_bytes());
    let uri = Uri::from_maybe_shared(Bytes::from(bytes)).map_err(|_| Error::InvalidRequest)?;
    let mut request = Request::new(body);
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    *request.headers_mut() = headers;
    validate_request(&request, Some(url))?;
    Ok(request)
}
/// Defend the transport boundary as well as the validated-spec caller. Resolved
/// environment values may occupy 32 separate 16 KiB fields; literal names have
/// the spec's aggregate 64 KiB cap. Default fields need four extra entries.
pub(crate) fn validate_request(request: &Request<Payload>, url: Option<&Url>) -> Result<(), Error> {
    let headers = request.headers();
    let uri = request.uri();
    if request.version() != hyper::http::Version::HTTP_11
        || request.method().as_str().len() > 64
        || uri.scheme().is_some()
        || uri.authority().is_some()
        || uri
            .path_and_query()
            .is_none_or(|part| part.as_str().len() > MAX_URL)
        || headers.len() > 36
    {
        return Err(Error::InvalidRequest);
    }
    let mut hosts = headers.get_all(header::HOST).iter();
    let host = hosts.next().ok_or(Error::InvalidRequest)?;
    if hosts.next().is_some()
        || host.as_bytes().is_empty()
        || url.is_some_and(|url| {
            host.as_bytes() != url[url::Position::BeforeHost..url::Position::AfterPort].as_bytes()
        })
        || [
            header::CONTENT_LENGTH,
            header::TRANSFER_ENCODING,
            header::CONNECTION,
            header::RANGE,
        ]
        .iter()
        .any(|name| headers.contains_key(name))
        || headers
            .get_all(header::ACCEPT_ENCODING)
            .iter()
            .any(|value| value != "identity")
        || headers.get_all(header::ACCEPT_ENCODING).iter().count() > 1
    {
        return Err(Error::InvalidRequest);
    }
    let mut names = 0_usize;
    for (name, value) in headers {
        if value.as_bytes().len() > MAX_URL {
            return Err(Error::InvalidRequest);
        }
        names = names
            .checked_add(name.as_str().len())
            .ok_or(Error::InvalidRequest)?;
        if names > 64 * 1_024 + 128 {
            return Err(Error::InvalidRequest);
        }
    }
    Ok(())
}
fn owned_value(bytes: &[u8]) -> Result<HeaderValue, Error> {
    let mut owned = Vec::new();
    owned
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Memory)?;
    owned.extend_from_slice(bytes);
    HeaderValue::from_maybe_shared(Bytes::from(owned)).map_err(|_| Error::InvalidRequest)
}
pub(crate) fn validate_url(url: &Url) -> Result<(), Error> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > MAX_URL
        || url.host_str().is_none_or(|host| host.len() > 253)
    {
        Err(Error::InvalidRequest)
    } else {
        Ok(())
    }
}
