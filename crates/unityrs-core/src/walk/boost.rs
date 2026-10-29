//! UnityPyBoost's reader (`UnityPyBoost/TypeTreeHelper.cpp:67-1056`), for
//! objects with `byte_size > 0`. The rules, each pinned by a test (plan
//! section 3.10, R1-R16): bounds checks `pos + n > end` evaluated even for
//! `n = 0`; `align4` from the object's start with no bounds check; strings
//! always aligned; `TypelessData` aligned only by its flag; the switch on the
//! node's own kind first, and only a kind it does not handle (`Unknown`,
//! `PPtr`, `Array`, `ReferencedObjectData`, `ManagedReferencesRegistry`) is an
//! array when its first child is `Array` (R9); primitive and `pair` elements
//! read as one run and align once, a `pair` run checking its arity before the
//! first element, so even when empty; other elements read and align one by
//! one; the `ManagedReferencesRegistry` scope; `ReferencedObject` data
//! resolved through the file's ref types in UnityPyBoost's order (R14: no
//! file, `ref_types` absent or not a list, no `type` read yet, a missing
//! `class`/`ns`/`asm`, all `ref_type`; then an empty class or a matched ref
//! type with no tree skips `data`; no match is `ref_type`); `PyDict_SetItem`
//! for repeated names; float32 widened (a signalling NaN quieted under
//! [`super::FloatMode::UnityPy`]).
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

/// Reads one value from exactly `data` (the object's bytes). `TypelessData`
/// and `vector<UInt8>` values are zero-copy slices of `data`.
pub fn read(data: &Bytes, tree: &Arc<Tree>, ctx: &ReadCtx<'_>) -> Result<Parsed, ReadError> {
    todo!()
}
