//! Reading typetree rows from a `SerializedFile` type record.
//!
//! - `parse_old` ports `TypeTreeNode.parse` (`helpers/TypeTreeNode.py:79-109`):
//!   explicit child counts read with UnityPy's stack, never recursion; a
//!   negative count means no children. Rows come out in pre-order with
//!   `level` = depth, `variable_count` at version 2, `index` and `meta_flag`
//!   except at version 3. Strings use `read_string_to_null` (the 32,767 window).
//! - `parse_blob` ports `TypeTreeNode.parse_blob` (`:111-154`): `node_count`,
//!   `stringbuffer_size`, rows unpacked as `hBBIIiii` plus `Q` from format 19
//!   (`:331-347`), so `version` is `i16` and `level`/`type_flags` are `u8`. The
//!   node area is read short at the end (memoryview), and a length that is not
//!   a whole number of rows raises, as `struct.iter_unpack` does. An offset with
//!   the high bit set indexes the whole common-string table
//!   ([`super::commonstring`]), whatever the engine version; an offset the
//!   table lacks becomes its decimal string (`:128-129`).
//!
//! Both return every stored row; [`super::NodeList::from_rows`] then builds the
//! kept tree. Format 10 and 12 and up use blobs (`SerializedFile.py`'s
//! `SerializedType`); the rest use the old format.
//!
//! Owner: package C.

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use crate::error::Error;
use crate::limits::Limits;
use crate::stream::Reader;
use crate::typetree::NodeRow;

/// Whether a `SerializedFile` of this format stores typetrees as blobs.
pub fn is_blob_format(format: u32) -> bool {
    format == 10 || format >= 12
}

/// `TypeTreeNode.parse`: an old-format tree, as pre-order rows.
pub fn parse_old(r: &mut Reader<'_>, format: u32, limits: &Limits) -> Result<Vec<NodeRow>, Error> {
    todo!()
}

/// `TypeTreeNode.parse_blob`: every stored row, in order.
pub fn parse_blob(r: &mut Reader<'_>, format: u32, limits: &Limits) -> Result<Vec<NodeRow>, Error> {
    todo!()
}
