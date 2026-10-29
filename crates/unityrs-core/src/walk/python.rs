//! UnityPy's pure-Python reader (`helpers/TypeTreeHelper.py:182-361`), for
//! objects with `byte_size == 0`. It starts at `byte_start` in the whole
//! `SerializedFile` and may read on past the object, with Python's reader
//! rules at any position ([`crate::stream`]'s helpers; the position may go
//! negative). **Alignment is relative to the file**, not the object:
//! `align_stream` rounds the absolute position. Its differences from
//! UnityPyBoost are `docs/CANONICAL_DUMP.md` section 7's list: aligned
//! primitive arrays become tuples, with 16-bit signedness swapped; aligned
//! `SInt8` and `TypelessData` arrays raise; a negative or out-of-range string
//! length reads as `""`; arrays of aligned classes align once and skip the
//! registry check; `TypelessData` length -1 reads to the end of the file and
//! other negative lengths follow Python slice semantics (so `bytes_read` can
//! be negative); a `pair` with three or more children reads its first two;
//! an aligned `pair` array reads its children without their own align flags
//! (`:285-293`); a `ReferencedObject` reads `type` before anything else (a
//! missing key is `KeyError`, `other`) and raises `SerializedFile has no
//! ref_types` for an empty or absent list *before* the empty-class skip (no
//! `ref_types` attribute is an `AttributeError` whose message the dump maps
//! to `ref_type`); an empty array of malformed `pair`s reads as `[]`; and an
//! aligned `ReferencedObject` array element whose class is empty or whose ref
//! type has no tree raises ([`super::ReadDetail::RefTypeNoTree`]). Each
//! difference is an oracle fixture of package B (plan section 7.3).
//!
//! Owner: package B.

// STUB(B): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::sync::Arc;

use bytes::Bytes;

use super::{ReadCtx, ReadError};
use crate::typetree::Tree;
use crate::value::Parsed;

/// Reads one value from `file`, starting at `start` (any Python position).
/// `bytes_read` counts from `start` and may be negative.
pub fn read(
    file: &Bytes,
    start: i128,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
) -> Result<Parsed, ReadError> {
    todo!()
}
