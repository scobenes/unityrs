//! UnityPy's pure-Python reader (`helpers/TypeTreeHelper.py:182-361`), for
//! objects with `byte_size == 0`. It starts at `byte_start` in the whole
//! `SerializedFile` and may read on past the object, with Python's reader
//! rules at any position ([`crate::stream`]'s helpers; the position may go
//! negative). **Alignment is relative to the file**, not the object:
//! `align_stream` rounds the absolute position. Its differences from
//! UnityPyBoost are `docs/CANONICAL_DUMP.md` section 7's list: aligned
//! primitive arrays become tuples, with 16-bit signedness swapped; an
//! aligned `SInt8` array raises at every length (`read_byte_array(reader,
//! size)` before any loop), and an array of aligned `TypelessData` raises
//! only when its length is above 0 (`read_bytes()` inside the loop; an empty
//! one reads as `[]`); a negative or out-of-range string
//! length reads as `""`, and a string of length 0 or read as `""` that way
//! is not aligned after (`streams/EndianBinaryReader.py:266-273` returns
//! before `align_stream`); in an array whose aligned element is a vector, a
//! negative inner length reads as `[]`, unless the inner element is an
//! aligned primitive, whose `struct` format rejects it (`struct.error`,
//! `other`); arrays of aligned classes align once and skip the
//! registry check; `TypelessData` length -1 reads to the end of the file and
//! other negative lengths follow Python slice semantics (so `bytes_read` can
//! be negative); a `pair` with three or more children reads its first two,
//! and an `Array` with three or more reads its elements through
//! `m_Children[1]` (`:216`, `:309`), where UnityPyBoost raises both;
//! an aligned `pair` array reads a child whose type is a key of
//! `FUNCTION_READ_MAP` (every primitive, `string`, `TypelessData`) through
//! that function, without the child's own align flag (a non-empty `string`
//! still aligns itself, as `read_aligned_string` does), and any other child
//! (a class, a vector, a `pair`) through `read_value`, which keeps its flag
//! (`:285-293`); a `ReferencedObject` reads `type` before anything else (a
//! missing key is `KeyError`, `other`; a `type` with no `class`, `ns` or
//! `asm` is `KeyError` and a non-dict `type` `AttributeError`, `:89-97`, both
//! `other` where UnityPyBoost reports `ref_type`) and raises `SerializedFile has no
//! ref_types` for an empty or absent list *before* the empty-class skip (no
//! `ref_types` attribute is an `AttributeError` whose message the dump maps
//! to `ref_type`); an aligned `pair` element with fewer than two children
//! raises `IndexError` (`other`) at every length, since `m_Children[0]` and
//! `[1]` are read before the loop (`:282-283`), while an empty array of
//! non-aligned malformed `pair`s, or of three-child `pair`s, reads as `[]`
//! (the probes: `CANONICAL_DUMP.md` section 7); likewise an aligned vector
//! element whose `Array` has fewer than two children raises `IndexError` at
//! every length, the empty one included, since `m_Children[0].m_Children[1]`
//! is read before the loop (`:309`), where UnityPyBoost reads an empty one
//! as `[]` (`read_value_array` reads an aligned element's `pair` children
//! and its vector's element before its loop, and indexes everywhere else
//! inside it); outside an array, a vector reads its length, and refuses a
//! negative one, before it indexes its `Array`'s element (`:213-216`), and a
//! `pair` reads its first child before it indexes its second (`:194-195`),
//! so a malformed one fails with whatever those reads raise first (a vector
//! whose `Array` has fewer than two children is `negative_length` at length
//! -1 and `IndexError`, `other`, at 0), where UnityPyBoost checks each arity
//! first and reports `other`: this reader never fails a compiled
//! `BadVector` or `BadPair` on entry; and an
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
