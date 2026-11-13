//! The response owns its unspawned hyper driver, socket and full backing buffers.
use super::{
    Detail, Error, Payload,
    permit::{Charge, RequestPermit},
    request::PIECE,
};
use hyper::{
    body::{Body, Bytes, Incoming},
    client::conn::http1,
    http::{HeaderMap, response::Parts},
};
use hyper_util::rt::TokioIo;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use unityrs_core::limits::heap_cost;

pub const HEAD_CAP: usize = 128 * 1_024;
/// Conservative reservation, including full hyper backing capacities, TLS
/// state/certificates and their growth overlaps. Counted tests are an additional
/// acceptance gate; this reservation alone does not prove the library's bound.
pub const LIBRARY_ENVELOPE: u64 = 3 * 1_024 * 1_024;
type Driver<I> = Pin<Box<http1::Connection<TokioIo<I>, Payload>>>;

/// No connection task or executor-owned future escapes this value.
pub struct Response<I: AsyncRead + AsyncWrite + Unpin> {
    parts: Parts,
    incoming: Option<Incoming>,
    driver: Option<Driver<I>>,
    driver_failed: bool,
    terminal_error: Option<Error>,
    pending: Bytes,
    envelope: Charge,
}
/// One independent, fallibly allocated buffer. Moving it to a spool write keeps
/// its original permit occupied and its full capacity charged until drop.
pub struct Data {
    bytes: Vec<u8>,
    _charge: Charge,
}
impl Data {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}
impl<I: AsyncRead + AsyncWrite + Unpin> Response<I> {
    #[must_use]
    pub fn status(&self) -> u16 {
        self.parts.status.as_u16()
    }
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }
    /// DATA only. Trailer frames are validated by hyper then discarded. No
    /// framing byte earns credit toward staging's body timers.
    pub async fn data(&mut self) -> Result<Option<Data>, Error> {
        if let Some(error) = self.terminal_error {
            return Err(error);
        }
        let result = self.next_data().await;
        if let Err(error) = result {
            self.terminal_error = Some(error);
            self.pending = Bytes::new();
            self.incoming.take();
            self.driver.take();
        }
        result
    }
    async fn next_data(&mut self) -> Result<Option<Data>, Error> {
        while self.pending.is_empty() {
            let Some(frame) = poll_fn(|cx| self.poll_frame(cx)).await? else {
                return Ok(None);
            };
            if let Ok(data) = frame.into_data() {
                self.pending = data;
            }
        }
        let count = self.pending.len().min(PIECE);
        let charge = self
            .envelope
            .additional(heap_cost(u64::try_from(count).map_err(|_| Error::Memory)?))
            .ok_or(Error::Memory)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(count).map_err(|_| Error::Memory)?;
        bytes.extend_from_slice(self.pending.get(..count).ok_or(Error::InvalidRequest)?);
        self.pending = self.pending.split_off(count);
        Ok(Some(Data {
            bytes,
            _charge: charge,
        }))
    }
    fn poll_frame(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<hyper::body::Frame<Bytes>>, Error>> {
        if let Some(driver) = self.driver.as_mut()
            && let Poll::Ready(result) = driver.as_mut().poll(cx)
        {
            self.driver_failed = result.is_err();
            self.driver.take();
        }
        let result = match self.incoming.as_mut() {
            Some(incoming) => Pin::new(incoming).poll_frame(cx),
            None => return Poll::Ready(Ok(None)),
        };
        match result {
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Ok(Some(frame))),
            Poll::Ready(Some(Err(_))) => self.end(Some(Error::transport(Detail::Truncated))),
            Poll::Ready(None) => self.end(None),
            Poll::Pending if self.driver_failed => {
                self.end(Some(Error::transport(Detail::Truncated)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn end(
        &mut self,
        error: Option<Error>,
    ) -> Poll<Result<Option<hyper::body::Frame<Bytes>>, Error>> {
        self.incoming.take();
        self.driver.take();
        Poll::Ready(error.map_or(Ok(None), Err))
    }
}
/// One HTTP/1.1 exchange over caller-supplied IO. Also used by deterministic
/// tests; production socket/TLS setup is owned by `Service`.
pub async fn exchange<I: AsyncRead + AsyncWrite + Unpin>(
    io: I,
    permit: &RequestPermit,
    request: hyper::Request<Payload>,
) -> Result<Response<I>, Error> {
    super::request::validate_request(&request, None)?;
    let envelope = permit.charge(LIBRARY_ENVELOPE).ok_or(Error::Memory)?;
    exchange_charged(io, envelope, request).await
}
pub(crate) async fn exchange_charged<I: AsyncRead + AsyncWrite + Unpin>(
    io: I,
    envelope: Charge,
    request: hyper::Request<Payload>,
) -> Result<Response<I>, Error> {
    let (mut sender, connection) = http1::Builder::new()
        .writev(true)
        .max_buf_size(HEAD_CAP)
        .max_header_size(HEAD_CAP)
        .handshake(TokioIo::new(io))
        .await
        .map_err(|e| super::classify::head(&e))?;
    let mut driver = Box::into_pin(crate::alloc::try_box(connection).map_err(|_| Error::Memory)?);
    let mut send = std::pin::pin!(sender.send_request(request));
    let mut done = false;
    let mut failed = false;
    let response = poll_fn(|cx| {
        let progress = if done {
            Poll::Pending
        } else {
            driver.as_mut().poll(cx)
        };
        if let Poll::Ready(result) = &progress {
            done = true;
            failed = result.is_err();
        }
        // A completed head/status wins over a later driver/body failure.
        if let Poll::Ready(response) = send.as_mut().poll(cx) {
            return Poll::Ready(response.map_err(|e| super::classify::head(&e)));
        }
        match progress {
            Poll::Ready(Err(error)) => Poll::Ready(Err(super::classify::head(&error))),
            Poll::Ready(Ok(())) => Poll::Ready(Err(Error::transport(Detail::NoResponse))),
            Poll::Pending => Poll::Pending,
        }
    })
    .await?;
    let (parts, incoming) = response.into_parts();
    Ok(Response {
        parts,
        incoming: Some(incoming),
        driver: if done { None } else { Some(driver) },
        driver_failed: failed,
        terminal_error: None,
        pending: Bytes::new(),
        envelope,
    })
}
