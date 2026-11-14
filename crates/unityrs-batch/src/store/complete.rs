//! A complete's 2xx body (`docs/plans/batch.md` 2.9). `object_store` reads
//! it whole and, finding `InternalError` or `SlowDown` anywhere in its raw
//! text, treats it as an error and quotes it (`body_contains_error` and
//! `retry_error_body`, `src/client/retry.rs`). So the connector reads it
//! first, at most [`SMALL_CAP`] bytes, into one buffer allocated once and
//! charged to the permit, and its root element decides: an `<Error>` root
//! fails the try, as a 2xx the call returns as an error, with the code it
//! names; any other root is the call's answer, whatever its text holds. In
//! that answer each word in character data gets its first letter as a
//! character reference (`&#73;nternalError`, `&#83;lowDown`), which the XML
//! parse reads back, so the `ETag` and key parse as sent and the raw text
//! no longer holds the word. A word anywhere else (a tag, an attribute, a
//! comment or a CDATA section) fails the try, since it would reach
//! `object_store`'s check as sent. So does a root that never closes, and
//! markup quick-xml's reader refuses before the root closed, on which
//! `object_store`'s parse would fail; after it, its parse reads nothing. The scan reads the
//! body as that reader does, a DOCTYPE's internal subset included, which a
//! test checks against quick-xml itself.
use super::{
    SMALL_CAP,
    attempt::{BodyFault, Fault, TrySlot},
    body::{Owned, Refusing, body_failed, content_length},
    scan::{CodeScan, Root},
};
use crate::http::{Response, permit::RequestPermit};
use hyper::body::Bytes;
use object_store::client::HttpResponseBody;
use tokio::io::{AsyncRead, AsyncWrite};
use unityrs_core::limits::heap_cost;

/// The words `object_store` looks for, each with its first letter as a
/// character reference. Neither word's first letter recurs in it.
const WORDS: [(&[u8], &[u8]); 2] = [(b"InternalError", b"&#73;"), (b"SlowDown", b"&#83;")];

/// The bytes a reference adds for the letter it replaces.
const GROWTH: usize = 4;

/// Matches [`WORDS`], byte by byte, across any chunking.
#[derive(Clone, Copy, Default)]
struct Words {
    matched: [usize; 2],
}
impl Words {
    /// Reads one byte; the word it ends, if any.
    fn feed(&mut self, b: u8) -> Option<usize> {
        let mut ended = None;
        for (i, ((word, _), matched)) in WORDS.iter().zip(self.matched.iter_mut()).enumerate() {
            if word.get(*matched) == Some(&b) {
                *matched += 1;
                if *matched == word.len() {
                    *matched = 0;
                    ended = Some(i);
                }
            } else {
                *matched = usize::from(word.first() == Some(&b));
            }
        }
        ended
    }
    fn clear(&mut self) {
        self.matched = [0; 2];
    }
}

/// How many words a body holds: in its character data, and in all.
fn count(body: &[u8]) -> (usize, usize, CodeScan) {
    let mut scan = CodeScan::default();
    let (mut text, mut raw) = (Words::default(), Words::default());
    let (mut in_text, mut in_raw) = (0, 0);
    scan.feed_text(body, |b, is_text| {
        in_raw += usize::from(raw.feed(b).is_some());
        if is_text {
            in_text += usize::from(text.feed(b).is_some());
        } else {
            text.clear();
        }
    });
    (in_text, in_raw, scan)
}

/// `body` with each word in its character data referenced, into `out`,
/// which holds room for it.
fn rewrite(body: &[u8], out: &mut Vec<u8>) {
    let mut scan = CodeScan::default();
    let mut text = Words::default();
    scan.feed_text(body, |b, is_text| {
        out.push(b);
        if !is_text {
            text.clear();
            return;
        }
        if let Some((word, reference)) = text.feed(b).and_then(|i| WORDS.get(i)) {
            let start = out.len().saturating_sub(word.len());
            out.truncate(start);
            out.extend_from_slice(reference);
            out.extend_from_slice(word.get(1..).unwrap_or_default());
        }
    });
}

/// Reads a complete's 2xx body and hands `object_store` the answer, or a
/// body that fails.
pub(super) async fn complete_body<I: AsyncRead + AsyncWrite + Unpin>(
    slot: &TrySlot,
    permit: &RequestPermit,
    response: &mut Response<I>,
) -> Result<HttpResponseBody, Fault> {
    let refuse = || HttpResponseBody::new(Refusing);
    let length = content_length(response);
    if length.is_some_and(|n| n > SMALL_CAP) {
        slot.update(|r| r.body = Some(BodyFault::OverCap));
        return Ok(refuse());
    }
    let capacity = length.unwrap_or(SMALL_CAP);
    let size = usize::try_from(capacity).map_err(|_| Fault::Internal)?;
    let charge = if size == 0 {
        None
    } else {
        Some(permit.charge(heap_cost(capacity)).ok_or(Fault::Memory)?)
    };
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Fault::Memory)?;
    loop {
        match response.data().await {
            Ok(Some(data)) => {
                let piece = data.as_slice();
                if piece.len() > size - bytes.len() {
                    slot.update(|r| r.body = Some(BodyFault::OverCap));
                    return Ok(refuse());
                }
                bytes.extend_from_slice(piece);
            }
            Ok(None) => break,
            Err(error) => {
                body_failed(slot, error);
                return Ok(refuse());
            }
        }
    }
    let (in_text, in_raw, scan) = count(&bytes);
    slot.update(|r| r.code = scan.code());
    // An answer is a root that closed, is no `<Error>`, and met no markup
    // the parse refuses before it closed; what follows it is never parsed.
    if scan.root() != Root::Other || !scan.closed() || scan.malformed() || in_raw > in_text {
        return Ok(refuse());
    }
    if in_text == 0 {
        return Ok(HttpResponseBody::from(Bytes::from_owner(Owned {
            bytes,
            _charge: charge,
        })));
    }
    let grown = bytes.len() + GROWTH * in_text;
    let grown_charge = permit
        .charge(heap_cost(grown as u64))
        .ok_or(Fault::Memory)?;
    let mut out = Vec::new();
    out.try_reserve_exact(grown).map_err(|_| Fault::Memory)?;
    rewrite(&bytes, &mut out);
    drop((bytes, charge));
    Ok(HttpResponseBody::from(Bytes::from_owner(Owned {
        bytes: out,
        _charge: Some(grown_charge),
    })))
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
