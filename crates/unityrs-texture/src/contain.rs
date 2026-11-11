//! Panic containment (`docs/plans/extract.md` E18): a decode that panics
//! becomes [`DecodeError::Panicked`], which the engine records as the
//! texture's `decoder_panic` skip (`docs/BATCH_SPEC.md` section 3), and the
//! batch goes on.
//!
//! The decoders do not panic on any input (indexing, `unwrap` and the rest
//! are denied, arithmetic is checked or bounded, and the oracle and seeded
//! tests feed them every size and every BC7 mode); this is the net under
//! that rule, as E18 asks. Unwinding returns everything the decode held: the
//! output buffer is dropped and its [`Charge`](unityrs_core::limits::Charge)
//! with it, so the meter is as it was before the call. Two things it cannot
//! do: catch a panic in a build with `panic = "abort"` (the workspace sets no
//! such profile), and stop the panic hook, which prints the panic to stderr
//! before the unwinding begins; an engine that wants its stderr clean sets
//! its own hook (`std::panic::set_hook`).

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::DecodeError;

/// The payload [`DecodeError::Panicked`] names when a panic's payload is
/// neither a `&str` nor a `String` (`docs/BATCH_SPEC.md` section 3).
pub const NON_STRING_PAYLOAD: &str = "non-string panic payload";

/// Runs `f`, a panic in it becoming [`DecodeError::Panicked`] with its
/// payload. `f` is asserted unwind-safe: what it borrows is only read, or
/// is a buffer it owns, dropped by the unwinding.
pub fn contain<T>(f: impl FnOnce() -> Result<T, DecodeError>) -> Result<T, DecodeError> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => Err(DecodeError::Panicked {
            payload: payload_text(payload.as_ref()),
        }),
    }
}

/// A panic's payload as text: a `&str` or `String` payload (what `panic!`
/// and the standard library's panics carry), else [`NON_STRING_PAYLOAD`].
fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        NON_STRING_PAYLOAD.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::limits::{Meter, MeteredVec};

    use super::{NON_STRING_PAYLOAD, contain};
    use crate::DecodeError;

    #[test]
    fn a_panic_is_an_error_with_its_payload() {
        let r: Result<(), _> = contain(|| panic!("boom {}", 7));
        assert_eq!(
            r,
            Err(DecodeError::Panicked {
                payload: "boom 7".to_owned()
            })
        );
        let r: Result<(), _> = contain(|| panic!("static"));
        assert_eq!(
            r,
            Err(DecodeError::Panicked {
                payload: "static".to_owned()
            })
        );
        let r: Result<(), _> = contain(|| std::panic::panic_any(42u8));
        assert_eq!(
            r,
            Err(DecodeError::Panicked {
                payload: NON_STRING_PAYLOAD.to_owned()
            })
        );
        #[allow(clippy::indexing_slicing)]
        let r: Result<u8, _> = contain(|| {
            let v: Vec<u8> = Vec::new();
            Ok(v[3])
        });
        assert!(matches!(r, Err(DecodeError::Panicked { .. })), "{r:?}");
    }

    #[test]
    fn what_is_returned_or_raised_passes_through() {
        assert_eq!(contain(|| Ok(5)), Ok(5));
        assert_eq!(
            contain::<()>(|| Err(DecodeError::NoImageData)),
            Err(DecodeError::NoImageData)
        );
    }

    /// The output's charge is returned by the unwinding: the meter is as it
    /// was.
    #[test]
    fn a_panic_returns_the_charge() {
        let meter = Meter::unlimited();
        let held = Arc::clone(&meter);
        let r: Result<(), _> = contain(move || {
            let v = MeteredVec::<u8>::with_capacity(&held, 1 << 20)?;
            assert!(held.used() >= 1 << 20);
            assert!(v.capacity() >= 1 << 20);
            panic!("after the charge");
        });
        assert!(matches!(r, Err(DecodeError::Panicked { .. })));
        assert_eq!(meter.used(), 0);
    }
}
