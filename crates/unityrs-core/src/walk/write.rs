//! UnityPy's pure-Python writer (`helpers/TypeTreeHelper.py:161-179`,
//! `:381-458`). Dispatch is on the type name first (`FUNCTION_WRITE_MAP`), then
//! `pair`, `ReferencedObject`, an `Array` first child, and a class. Under
//! [`super::WriteRules::UnityPy`] each array element aligns after itself when
//! its node is flagged, then the vector or `Array` flag aligns once; a class
//! writes `value[child.m_Name]` for every child, a missing key failing and an
//! extra key ignored; `ReferencedObject` data needs a non-empty `ref_types`
//! and a `data` entry; integers are range-checked as `struct.pack` checks them.
//!
//! Owner: package B.

// STUB(B): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::ptr_arg,
    clippy::missing_panics_doc,
    unused_variables
)]

use super::{WriteCtx, WriteError};
use crate::typetree::Tree;
use crate::value::Parsed;

/// Writes `parsed` through `tree` (the object's own tree, or a `nodes=`
/// override: keys are then matched by name), appending to `out`.
pub fn write(
    parsed: &Parsed,
    tree: &Tree,
    ctx: &WriteCtx<'_>,
    out: &mut Vec<u8>,
) -> Result<(), WriteError> {
    todo!()
}
