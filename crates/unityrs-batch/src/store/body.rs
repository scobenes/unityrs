//! The bodies the connector hands `object_store` (`docs/plans/batch.md`
//! 2.9). A non-2xx body is read by the connector, at most 64 KiB, into one
//! buffer allocated once and charged to the permit, and cut there. A 2xx
//! body passes through, chunk by chunk and uncopied, under its kind's cap,
//! with a size hint clamped to that cap, so no server's `Content-Length`
//! sizes an allocation: past the cap a head GET's read stops, any other's
//! fails. A LIST page also passes the tag scan, and every body the `<Code>`
//! scan. Past a cap or a breach nothing more is read.
use super::{
    ERROR_CAP, Kind, LIST_CAP,
    attempt::{BodyFault, Fault, TrySlot},
    connector::{decimal, refused},
    scan::{CodeScan, TagScan},
};
use crate::http::{self, Data, Response, permit::Charge, permit::RequestPermit};
use hyper::{
    body::{Body, Bytes, Frame, SizeHint},
    http::header::CONTENT_LENGTH,
};
use object_store::client::{HttpError, HttpResponseBody};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use unityrs_core::limits::heap_cost;

/// A LIST page's most `<Contents>` (`max_keys`).
const MAX_KEYS: u32 = 1_000;

fn content_length<I: AsyncRead + AsyncWrite + Unpin>(response: &Response<I>) -> Option<u64> {
    let mut values = response.headers().get_all(CONTENT_LENGTH).iter();
    let first = values.next()?;
    if values.next().is_some() {
        return None;
    }
    decimal(first.as_bytes())
}

/// The charged buffer a non-2xx body is read into, uncopied into `Bytes`.
struct Owned {
    bytes: Vec<u8>,
    _charge: Option<Charge>,
}
impl AsRef<[u8]> for Owned {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// A DATA piece of the service's, uncopied into `Bytes`; its charge lasts
/// as long as `object_store` holds it.
struct Piece(Data);
impl AsRef<[u8]> for Piece {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Reads at most [`ERROR_CAP`] bytes of a non-2xx body. A body that fails
/// leaves what arrived; the try keeps its status's row.
pub(super) async fn error_body<I: AsyncRead + AsyncWrite + Unpin>(
    slot: &TrySlot,
    permit: &RequestPermit,
    response: &mut Response<I>,
) -> Result<HttpResponseBody, Fault> {
    let capacity = content_length(response).map_or(ERROR_CAP, |n| n.min(ERROR_CAP));
    let size = usize::try_from(capacity).map_err(|_| Fault::Internal)?;
    let charge = if size == 0 {
        None
    } else {
        Some(permit.charge(heap_cost(capacity)).ok_or(Fault::Memory)?)
    };
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Fault::Memory)?;
    let mut code = CodeScan::default();
    while bytes.len() < size {
        match response.data().await {
            Ok(Some(data)) => {
                let piece = data.as_slice();
                let take = piece
                    .get(..piece.len().min(size - bytes.len()))
                    .unwrap_or_default();
                code.feed(take);
                bytes.extend_from_slice(take);
            }
            Ok(None) => break,
            Err(error) => {
                body_failed(slot, error);
                break;
            }
        }
    }
    slot.update(|r| r.code = code.code());
    Ok(HttpResponseBody::from(Bytes::from_owner(Owned {
        bytes,
        _charge: charge,
    })))
}

/// Preflight's bucket proof: its 2xx body is read under the LIST cap and
/// dropped, so `object_store`'s parse sees none of it (7.2).
pub(super) async fn drain<I: AsyncRead + AsyncWrite + Unpin>(
    slot: &TrySlot,
    response: &mut Response<I>,
) -> HttpResponseBody {
    let mut read = 0_u64;
    while read <= LIST_CAP {
        match response.data().await {
            Ok(Some(data)) => read += data.as_slice().len() as u64,
            Ok(None) => break,
            Err(error) => {
                body_failed(slot, error);
                break;
            }
        }
    }
    HttpResponseBody::from(Bytes::new())
}

fn body_failed(slot: &TrySlot, error: http::Error) {
    match error {
        http::Error::Transport(failure) => {
            slot.update(|r| r.body = Some(BodyFault::Transport(failure.detail)));
        }
        http::Error::Memory => slot.fail(Fault::Memory),
        _ => slot.fail(Fault::Internal),
    }
}

type Read<I> =
    Pin<Box<dyn Future<Output = (Response<I>, Result<Option<Data>, http::Error>)> + Send>>;

struct Inner<I: AsyncRead + AsyncWrite + Unpin> {
    response: Option<Response<I>>,
    read: Option<Read<I>>,
    slot: Arc<TrySlot>,
    kind: Kind,
    passed: u64,
    scan: Option<TagScan>,
    code: CodeScan,
    done: bool,
}

/// A 2xx body under its kind's cap. The mutex makes it `Sync`, which
/// `object_store` asks of a body; it is never contended.
pub(super) struct Capped<I: AsyncRead + AsyncWrite + Unpin> {
    inner: Mutex<Inner<I>>,
    hint: SizeHint,
}

pub(super) fn capped<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    slot: &Arc<TrySlot>,
    kind: Kind,
    response: Response<I>,
) -> HttpResponseBody {
    let cap = kind.cap();
    let mut hint = SizeHint::new();
    match content_length(&response) {
        Some(length) if length <= cap => hint.set_exact(length),
        length => {
            hint.set_lower(length.unwrap_or(0).min(cap));
            hint.set_upper(cap);
        }
    }
    HttpResponseBody::new(Capped {
        inner: Mutex::new(Inner {
            response: Some(response),
            read: None,
            slot: Arc::clone(slot),
            kind,
            passed: 0,
            scan: (kind == Kind::ListPage).then(|| TagScan::new(MAX_KEYS)),
            code: CodeScan::default(),
            done: false,
        }),
        hint,
    })
}

impl<I: AsyncRead + AsyncWrite + Unpin + Send + 'static> Inner<I> {
    fn end(&mut self, fault: Option<BodyFault>) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        self.done = true;
        self.response = None;
        self.read = None;
        let code = self.code.code();
        self.slot.update(|r| {
            r.code = code;
            if fault.is_some() {
                r.body = fault;
            }
        });
        Poll::Ready(fault.map(|_| Err(refused())))
    }

    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        if self.done {
            return Poll::Ready(None);
        }
        if self.read.is_none() {
            let Some(mut response) = self.response.take() else {
                return self.end(None);
            };
            let read = crate::alloc::try_box(async move {
                let data = response.data().await;
                (response, data)
            });
            let Ok(read) = read else {
                self.slot.fail(Fault::Memory);
                let _ = self.end(None);
                return Poll::Ready(Some(Err(refused())));
            };
            self.read = Some(Box::into_pin(read));
        }
        let Some(read) = self.read.as_mut() else {
            return self.end(None);
        };
        let Poll::Ready((response, data)) = read.as_mut().poll(cx) else {
            return Poll::Pending;
        };
        self.read = None;
        let data = match data {
            Ok(Some(data)) => data,
            Ok(None) => return self.end(None),
            Err(http::Error::Transport(failure)) => {
                return self.end(Some(BodyFault::Transport(failure.detail)));
            }
            Err(error) => {
                body_failed(&self.slot, error);
                let _ = self.end(None);
                return Poll::Ready(Some(Err(refused())));
            }
        };
        let piece = data.as_slice();
        let length = piece.len() as u64;
        let cap = self.kind.cap();
        self.code.feed(piece);
        if self.passed + length > cap {
            if self.kind != Kind::HeadRead {
                return self.end(Some(BodyFault::OverCap));
            }
            // A head's read stops at its cap: the head is unreadable.
            let keep = usize::try_from(cap - self.passed).unwrap_or(0);
            self.passed = cap;
            drop(response);
            let bytes = Bytes::from_owner(Piece(data)).slice(..keep);
            let _ = self.end(None);
            return Poll::Ready((keep > 0).then(|| Ok(Frame::data(bytes))));
        }
        if let Some(scan) = self.scan.as_mut()
            && scan.feed(piece).is_err()
        {
            return self.end(Some(BodyFault::Scan));
        }
        self.passed += length;
        self.response = Some(response);
        Poll::Ready(Some(Ok(Frame::data(Bytes::from_owner(Piece(data))))))
    }
}

impl<I: AsyncRead + AsyncWrite + Unpin + Send + 'static> Body for Capped<I> {
    type Data = Bytes;
    type Error = HttpError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        self.get_mut()
            .inner
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .poll_next(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .done
    }
    fn size_hint(&self) -> SizeHint {
        self.hint
    }
}
