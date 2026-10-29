// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
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
//!   node area and the string buffer are `reader.read(n)`: a Python slice,
//!   short at the end, and for a negative `node_count` or `stringbuffer_size`
//!   a slice that moves the position back ([`crate::stream::py_read`]); a node
//!   area that is not a whole number of rows raises, as `struct.iter_unpack`
//!   does. An offset with
//!   the high bit set indexes the whole common-string table, whatever the
//!   engine version: the caller's TPK's ([`super::commonstring::CommonStrings`],
//!   UnityPy's `get_common_strings()`), or without one UnityPy's MIT
//!   [`super::commonstring::COMMON_STRINGS`]; an offset the table lacks
//!   becomes its decimal string (`:128-129`).
//!
//! Both return every stored row; [`super::NodeList::from_rows`] then builds the
//! kept tree. Format 10 and 12 and up use blobs (`SerializedFile.py`'s
//! `SerializedType`); the rest use the old format.
//!
//! **Bounds.** Rows are pushed into a [`MeteredVec`] on the input's
//! [`Meter`], so every growth of the row table is charged before it happens
//! and a refused one allocates nothing; growth follows the rows actually
//! read, never `node_count`. Node names and types are zero-copy slices of the
//! string buffer ([`crate::raw::RawStr::from_shared`]) or static common
//! strings, so such a row costs `size_of::<NodeRow>()` whatever its strings'
//! lengths. The one string a row computes, an unknown common offset's
//! decimal string, is made with [`crate::raw::RawStr::charged`] on the same
//! meter (its buffer and its owner record, carried inside the string, so a
//! clone that outlives the row still counts); a parse may share one such
//! string between rows naming the same offset. A file that declares far more
//! rows than it is long (LZMA compresses repeated rows well) is stopped by
//! the meter, not by a count.
//!
//! Owner: package C.

use std::sync::Arc;

use bytes::Bytes;

use crate::error::Error;
use crate::limits::{Limits, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::stream::{Endian, Reader, STRING_TO_NULL_WINDOW, py_read, py_string_to_null};
use crate::typetree::NodeRow;
use crate::typetree::commonstring::{CommonStrings, common_string};

/// Whether a `SerializedFile` of this format stores typetrees as blobs.
#[must_use]
pub fn is_blob_format(format: u32) -> bool {
    // UnityPy files/SerializedFile.py:137
    format == 10 || format >= 12
}

/// A zero-copy string of `data` at `range`.
fn shared(data: &Bytes, range: std::ops::Range<usize>) -> RawStr {
    if range.start <= range.end && range.end <= data.len() {
        RawStr::from_shared(data.slice(range))
    } else {
        RawStr::default()
    }
}

/// `read_string_to_null()` on `r` as a zero-copy slice of `data` (the
/// buffer `r` reads).
pub(crate) fn string_to_null(r: &mut Reader<'_>, data: &Bytes) -> Result<RawStr, Error> {
    match py_string_to_null(r.data(), r.position(), STRING_TO_NULL_WINDOW) {
        Some((range, new)) => {
            r.set_position(new);
            Ok(shared(data, range))
        }
        // The reader's own read raises the same way.
        None => r.read_string_to_null().map(|_| RawStr::default()),
    }
}

/// `TypeTreeNode.parse`: an old-format tree, as pre-order rows. `data` is
/// the buffer `r` reads, so strings are slices of it.
pub fn parse_old(
    r: &mut Reader<'_>,
    data: &bytes::Bytes,
    format: u32,
    meter: &Arc<Meter>,
    limits: &Limits,
) -> Result<MeteredVec<NodeRow>, Error> {
    let _ = limits;
    let mut rows: MeteredVec<NodeRow> = MeteredVec::new(meter);
    // UnityPy helpers/TypeTreeNode.py:83-109: `(parent level, children left)`
    // for every node whose children are still being read, the hidden root
    // (level -1, one child) first.
    let mut stack: MeteredVec<(i32, i32)> = MeteredVec::new(meter);
    stack.push((-1, 1))?;
    while let Some(&(parent_level, count)) = stack.as_slice().last() {
        if count == 1 {
            stack.truncate(stack.len() - 1);
        } else if let Some(top) = stack.as_mut_slice().last_mut() {
            top.1 = count - 1;
        }
        let level = parent_level.saturating_add(1);
        let ty = string_to_null(r, data)?;
        let name = string_to_null(r, data)?;
        let byte_size = r.read_i32()?;
        let variable_count = if format == 2 {
            Some(r.read_i32()?)
        } else {
            None
        };
        let index = if format == 3 {
            None
        } else {
            Some(r.read_i32()?)
        };
        let type_flags = r.read_i32()?;
        let version = r.read_i32()?;
        let meta_flag = if format == 3 {
            None
        } else {
            Some(i64::from(r.read_i32()?))
        };
        rows.push(NodeRow {
            level,
            ty,
            name,
            byte_size,
            version,
            type_flags: Some(type_flags),
            variable_count,
            index,
            meta_flag,
            ref_type_hash: None,
        })?;
        let children = r.read_i32()?;
        if children > 0 {
            stack.push((level, children))?;
        }
    }
    Ok(rows)
}

/// The size of one blob row: `hBBIIiii`, plus `Q` from format 19
/// (`helpers/TypeTreeNode.py:331-347`).
fn blob_row_size(format: u32) -> usize {
    if format >= 19 { 32 } else { 24 }
}

/// A blob row's fields in the file's byte order.
struct BlobRow<'a> {
    row: &'a [u8],
    endian: Endian,
}

impl BlobRow<'_> {
    fn bytes<const N: usize>(&self, at: usize) -> [u8; N] {
        let mut b = self
            .row
            .get(at..at + N)
            .and_then(|b| <[u8; N]>::try_from(b).ok())
            .unwrap_or([0; N]);
        if self.endian == Endian::Big {
            b.reverse();
        }
        b
    }

    fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.bytes(at))
    }

    fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.bytes(at))
    }
}

/// `TypeTreeNode.parse_blob`: every stored row, in order. `data` is the
/// buffer `r` reads, so local strings are slices of its string buffer.
/// `common` is the common-string table of the caller's TPK
/// ([`crate::provider::ProviderChain::common_strings`]); `None` reads
/// [`super::commonstring::COMMON_STRINGS`].
pub fn parse_blob(
    r: &mut Reader<'_>,
    data: &bytes::Bytes,
    format: u32,
    meter: &Arc<Meter>,
    limits: &Limits,
    common: Option<&CommonStrings>,
) -> Result<MeteredVec<NodeRow>, Error> {
    let _ = limits;
    // UnityPy helpers/TypeTreeNode.py:113-118
    let node_count = r.read_i32()?;
    let buffer_size = r.read_i32()?;
    let size = blob_row_size(format);
    let row_bytes = i64::from(node_count).saturating_mul(i64::try_from(size).unwrap_or(32));
    let (rows_range, pos) = py_read(data.len(), r.position(), row_bytes);
    r.set_position(pos);
    let (buf_range, pos) = py_read(data.len(), r.position(), i64::from(buffer_size));
    r.set_position(pos);
    let rows_data = data.get(rows_range).unwrap_or_default();
    // `struct.iter_unpack` raises on a buffer that is not whole rows.
    if rows_data.len() % size != 0 {
        return Err(Error::Malformed {
            what: "typetree blob",
            detail: format!(
                "iterative unpacking requires a buffer of a multiple of {size} bytes, got {}",
                rows_data.len()
            )
            .into(),
        });
    }
    let buffer = shared(data, buf_range).into_bytes();
    let endian = r.endian();
    // Growth follows the rows present, never `node_count`.
    let mut rows: MeteredVec<NodeRow> = MeteredVec::with_capacity(meter, rows_data.len() / size)?;
    let mut last_unknown: Option<(u32, RawStr)> = None;
    for row in rows_data.chunks_exact(size) {
        let b = BlobRow { row, endian };
        let version = i16::from_le_bytes(b.bytes(0));
        let level = row.get(2).copied().unwrap_or(0);
        let type_flags = row.get(3).copied().unwrap_or(0);
        // UnityPy helpers/TypeTreeNode.py:135-138: the type before the name.
        let ty = blob_string(&buffer, b.u32(4), common, meter, &mut last_unknown)?;
        let name = blob_string(&buffer, b.u32(8), common, meter, &mut last_unknown)?;
        rows.push(NodeRow {
            level: i32::from(level),
            ty,
            name,
            byte_size: b.i32(12),
            version: i32::from(version),
            type_flags: Some(i32::from(type_flags)),
            variable_count: None,
            index: Some(b.i32(16)),
            meta_flag: Some(i64::from(b.i32(20))),
            ref_type_hash: (format >= 19).then(|| u64::from_le_bytes(b.bytes(24))),
        })?;
    }
    Ok(rows)
}

/// `read_string` in `parse_blob` (`helpers/TypeTreeNode.py:122-129`): a
/// local offset reads to a NUL in the string buffer from there (a reader
/// over the buffer, the 32,767-byte window); a high-bit offset is the common
/// string (`common`'s, or [`super::commonstring::COMMON_STRINGS`]'s), or the
/// offset's decimal string when the table has none, the last such string
/// shared with the next row that names it.
fn blob_string(
    buffer: &Bytes,
    value: u32,
    common: Option<&CommonStrings>,
    meter: &Arc<Meter>,
    last_unknown: &mut Option<(u32, RawStr)>,
) -> Result<RawStr, Error> {
    if value & 0x8000_0000 == 0 {
        let (range, _) = py_string_to_null(buffer, i128::from(value), STRING_TO_NULL_WINDOW)
            .ok_or_else(|| Error::Malformed {
                what: "typetree string",
                detail: format!("String not terminated at offset {value}").into(),
            })?;
        return Ok(shared(buffer, range));
    }
    let offset = value & 0x7FFF_FFFF;
    match common {
        Some(table) => {
            if let Some(s) = table.get(offset) {
                return Ok(s.clone());
            }
        }
        None => {
            if let Some(s) = common_string(offset) {
                return Ok(RawStr::from_static(s));
            }
        }
    }
    if let Some((o, s)) = last_unknown
        && *o == offset
    {
        return Ok(s.clone());
    }
    // `str(offset)`, written on the stack: the only heap is the charged copy.
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut n = offset;
    loop {
        at -= 1;
        if let Some(d) = digits.get_mut(at) {
            // `n % 10` is below 10.
            *d = b'0' + u8::try_from(n % 10).unwrap_or(0);
        }
        n /= 10;
        if n == 0 || at == 0 {
            break;
        }
    }
    let s = RawStr::charged(digits.get(at..).unwrap_or_default(), meter)?;
    *last_unknown = Some((offset, s.clone()));
    Ok(s)
}
