//! UnityPy's pure-Python writer (`helpers/TypeTreeHelper.py:161-179`,
//! `:381-458`). Dispatch is on the type name first (`FUNCTION_WRITE_MAP`), then
//! `pair`, `ReferencedObject`, an `Array` first child, and a class. Under
//! [`super::WriteRules::UnityPy`] each array element aligns after itself when
//! its node is flagged, then the vector or `Array` flag aligns once; a class
//! writes `value[child.m_Name]` for every child, a missing key failing and an
//! extra key ignored; `ReferencedObject` data needs a non-empty `ref_types`
//! (`:99-100`) and a ref type with a tree: an empty class, or a matched ref
//! type with no tree, fails (`KeyError: 'data'` when the reader left `data`
//! out, else an `AttributeError` on the `None` node); integers are
//! range-checked as `struct.pack` checks them.
//!
//! **Duck typing (W2).** The writer takes any value Python's operations
//! accept, not only the variant the reader makes for the node, which a
//! duplicate-name object exercises (its dict holds one child's value under
//! the other's node). Probed on UnityPy 1.25.2: a `bool` node takes any value
//! (its truthiness; an empty `Str`, `Bytes`, `List`, `Tuple` or `Dict` is
//! false); an integer node takes `Bool` and `Int`/`UInt` in range; a `float`
//! or `double` node takes `Bool`, integers and floats; a `string` node only
//! `Str`; `TypelessData` only `Bytes`; a `pair` node anything with items 0
//! and 1 (`List`, `Tuple`, `Prims` or `Bytes` of length 2 or more, a `Str`
//! of 2 or more characters written through the children's kinds); an array
//! any sized iterable (`List`, `Tuple`, `Prims`, `Bytes` as integers, `Str`
//! as one-character strings, a `Dict` as its keys), each element written by
//! the element node under the same rules; a class only a `Dict`.
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
use crate::budget::DecodeBuf;
use crate::typetree::Tree;
use crate::value::Parsed;

/// Writes `parsed` through `tree` (the object's own tree, or a `nodes=`
/// override: keys are then matched by name), appending to `out`, a buffer
/// on the object's input's meter that charges before it grows (a value read
/// with a negative length can name the same bytes many times, so the output
/// can be far larger than the value; plan section 3.15).
pub fn write(
    parsed: &Parsed,
    tree: &Tree,
    ctx: &WriteCtx<'_>,
    out: &mut DecodeBuf,
) -> Result<(), WriteError> {
    todo!()
}
