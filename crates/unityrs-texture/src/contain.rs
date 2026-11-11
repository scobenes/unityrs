//! Panic containment (`docs/plans/extract.md` E18): a decode that panics
//! becomes [`DecodeError::Panicked`], which the engine records as the
//! texture's `decoder_panic` skip (`docs/BATCH_SPEC.md` section 3), and the
//! batch goes on.
//!
//! The decoders do not panic on any input (indexing, `unwrap` and the rest
//! are denied, arithmetic is checked or bounded, and the oracle and seeded
//! tests feed them every size and every BC7 mode); this is the net under
//! that rule, as E18 asks. [`crate::decode()`] runs only its pixel loop
//! under it: the checks and the output's charge and allocation come before,
//! outside it, so a panic there (a panicking `MemoryBudget`, say) is not
//! taken for the decoder's, and the meter's own refund on a refusal is never
//! cut short. Unwinding out of the loop drops the image being written, its
//! buffer first and then its [`Charge`](unityrs_core::limits::Charge), so
//! the meter is as it was before the call.
//!
//! The payload is kept as the unwinding carried it ([`Payload`]): nothing is
//! copied or allocated on the panic path, whose payload may be as long as
//! anything a decoder could format from its input, and which no meter has
//! charged.
//!
//! Two things it cannot do: catch a panic in a build with
//! `panic = "abort"` (the workspace sets no such profile), and stop the
//! panic hook, which prints the panic to stderr before the unwinding begins;
//! an engine that wants its stderr clean sets its own hook
//! (`std::panic::set_hook`).

use std::any::Any;
use std::fmt;
use std::panic::{UnwindSafe, catch_unwind};
use std::sync::{Mutex, PoisonError};

use crate::DecodeError;

/// The text [`Payload::message`] gives when a panic's payload is neither a
/// `&str` nor a `String` (`docs/BATCH_SPEC.md` section 7.3).
pub const NON_STRING_PAYLOAD: &str = "non-string panic payload";

/// The most bytes [`Payload::message`] gives: a `decoder_panic`'s detail is
/// the payload cut to its first 1,024 bytes at a code point boundary
/// (`docs/BATCH_SPEC.md` section 7.3).
pub const MESSAGE_BYTES: usize = 1024;

/// Runs `f`, a panic in it becoming [`DecodeError::Panicked`] with its
/// payload. `f` must be [`UnwindSafe`]: a caller whose closure writes into
/// a buffer it then drops on the error asserts so itself
/// ([`std::panic::AssertUnwindSafe`]), as [`crate::decode()`] does.
pub fn contain<T, F>(f: F) -> Result<T, DecodeError>
where
    F: FnOnce() -> Result<T, DecodeError> + UnwindSafe,
{
    match catch_unwind(f) {
        Ok(result) => result,
        Err(payload) => Err(DecodeError::Panicked {
            payload: Payload::new(payload),
        }),
    }
}

/// A panic's payload, kept whole, with no copy: the box the unwinding
/// carried, downcast where it holds a `&str` or a `String` (what `panic!`
/// and the standard library's panics carry), which moves nothing.
pub struct Payload(Kept);

// The payload's own boxes, downcast: unboxing them would free each box,
// and `into_any` would then allocate a new one.
#[allow(clippy::redundant_allocation, clippy::box_collection)]
enum Kept {
    Str(Box<&'static str>),
    String(Box<String>),
    /// Any other payload. The mutex only makes the error `Sync`, as the
    /// payload need not be; it is never locked.
    Other(Mutex<Box<dyn Any + Send>>),
}

const _: () = crate::send_sync::<Payload>();

impl Payload {
    fn new(payload: Box<dyn Any + Send>) -> Self {
        let payload = match payload.downcast::<&'static str>() {
            Ok(s) => return Self(Kept::Str(s)),
            Err(other) => other,
        };
        match payload.downcast::<String>() {
            Ok(s) => Self(Kept::String(s)),
            Err(other) => Self(Kept::Other(Mutex::new(other))),
        }
    }

    /// The whole text: the payload's, or [`NON_STRING_PAYLOAD`].
    fn text(&self) -> &str {
        match &self.0 {
            Kept::Str(s) => s,
            Kept::String(s) => s,
            Kept::Other(_) => NON_STRING_PAYLOAD,
        }
    }

    /// The payload's text, or [`NON_STRING_PAYLOAD`], cut to its first
    /// [`MESSAGE_BYTES`] bytes at a code point boundary: a `decoder_panic`'s
    /// detail.
    #[must_use]
    pub fn message(&self) -> &str {
        let text = self.text();
        text.get(..text.floor_char_boundary(MESSAGE_BYTES))
            .unwrap_or(text)
    }

    /// Whether [`Self::message`] is cut: the manifest row's `cut`.
    #[must_use]
    pub fn is_cut(&self) -> bool {
        self.text().len() > MESSAGE_BYTES
    }

    /// The payload as the unwinding carried it, for
    /// [`std::panic::resume_unwind`].
    #[must_use]
    pub fn into_any(self) -> Box<dyn Any + Send> {
        match self.0 {
            Kept::Str(s) => s,
            Kept::String(s) => s,
            Kept::Other(m) => m.into_inner().unwrap_or_else(PoisonError::into_inner),
        }
    }
}

/// Two payloads are equal when their texts are.
impl PartialEq for Payload {
    fn eq(&self, other: &Self) -> bool {
        self.text() == other.text()
    }
}

impl Eq for Payload {}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Payload").field(&self.message()).finish()
    }
}

/// [`Payload::message`].
impl fmt::Display for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::{MESSAGE_BYTES, NON_STRING_PAYLOAD, Payload, contain};
    use crate::DecodeError;

    fn payload<T: std::fmt::Debug>(r: Result<T, DecodeError>) -> Payload {
        match r {
            Err(DecodeError::Panicked { payload }) => payload,
            other => panic!("not a panic: {other:?}"),
        }
    }

    #[test]
    fn a_panic_is_an_error_with_its_payload() {
        let p = payload(contain::<(), _>(|| panic!("boom {}", 7)));
        assert_eq!((p.message(), p.is_cut()), ("boom 7", false));
        let p = payload(contain::<(), _>(|| panic!("static")));
        assert_eq!(p.message(), "static");
        let p = payload(contain::<(), _>(|| std::panic::panic_any(42u8)));
        assert_eq!(p.message(), NON_STRING_PAYLOAD);
        assert_eq!(p.into_any().downcast_ref::<u8>(), Some(&42));
        #[allow(clippy::indexing_slicing)]
        let r: Result<u8, _> = contain(|| {
            let v: Vec<u8> = Vec::new();
            Ok(v[3])
        });
        assert!(matches!(r, Err(DecodeError::Panicked { .. })), "{r:?}");
    }

    /// The payload's own bytes, not a copy, cut at a code point boundary.
    #[test]
    fn the_payload_is_kept_and_its_message_cut() {
        let text = format!("a{}", "é".repeat(MESSAGE_BYTES));
        let at = text.as_ptr();
        let p = payload(contain::<(), _>(move || std::panic::panic_any(text)));
        assert_eq!(p.message().as_ptr(), at);
        // "a", then two-byte code points: byte 1,024 is inside one.
        assert_eq!(p.message().len(), MESSAGE_BYTES - 1);
        assert!(p.is_cut());
        assert_eq!(p.to_string(), p.message());
        let whole = p.into_any().downcast::<String>().unwrap();
        assert_eq!((whole.as_ptr(), whole.len()), (at, 1 + 2 * MESSAGE_BYTES));
        let exact = "x".repeat(MESSAGE_BYTES);
        let p = payload(contain::<(), _>(move || std::panic::panic_any(exact)));
        assert_eq!((p.message().len(), p.is_cut()), (MESSAGE_BYTES, false));
    }

    #[test]
    fn what_is_returned_or_raised_passes_through() {
        assert_eq!(contain(|| Ok(5)), Ok(5));
        assert_eq!(
            contain::<(), _>(|| Err(DecodeError::NoImageData)),
            Err(DecodeError::NoImageData)
        );
    }
}
