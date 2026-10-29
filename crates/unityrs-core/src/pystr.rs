//! Python string semantics on raw bytes, as UnityPy applies them to names.
//!
//! UnityPy compares CAB and resource names after `ntpath.basename` and
//! `str.lower()` (`environment.py:357-363`, `classes/PPtr.py:66-77`,
//! `helpers/ResourceReader.py:11-18`). Python lower-cases a surrogate-escaped
//! string: valid UTF-8 runs with Unicode full case mapping (Python 3.11 has
//! Unicode 14.0), invalid bytes passed through. Rust's tables are newer, so a
//! generated exception table ([`crate::tables::lower_exception`]) pins the code
//! points whose mapping changed after 14.0 (`tools/gen/`, package A; plan risk R7).
//!
//! **Memory.** Each function that returns a string computes it from an
//! input's name (a directory path, an external path, a resource path, up to
//! the input's size) and charges it to that input's meter before it is
//! allocated ([`RawStr::charged`]): a refusal allocates nothing. A result
//! equal to its input (a name already lower-case and without a directory)
//! may be returned as the input's own bytes, uncopied. Comparisons that only
//! need to know whether two names are equal should compare without building
//! the lowered string at all.
//!
//! Owner: package D.

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::sync::Arc;

use crate::limits::{ChargeError, Meter};
use crate::raw::RawStr;

/// `str.lower()` on surrogate-escaped bytes, charged to `meter`.
pub fn py_lower(s: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    todo!()
}

/// `ntpath.basename`: after `splitdrive`, the part after the last `/` or `\`.
pub fn ntpath_basename(p: &[u8]) -> &[u8] {
    todo!()
}

/// `ntpath.splitext`: `(root, ext)`, leading dots not counting as an extension.
pub fn ntpath_splitext(p: &[u8]) -> (&[u8], &[u8]) {
    todo!()
}

/// `simplify_name`: `ntpath.basename(name).lower()`, the CAB registry's key,
/// charged to `meter`.
pub fn simplify_name(name: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    todo!()
}

/// `PPtr.deref`'s external path cleaning: strip a leading `archive:/`, then a
/// leading `assets/`, keep what follows the last `/`, lower-case it; charged
/// to `meter`.
pub fn clean_external_path(path: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    todo!()
}
