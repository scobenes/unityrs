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
//! **Work.** A string's search reads up to 32,767 bytes and consumes only the
//! string, and a parse's position can move back (a blob's negative
//! `node_count` or `stringbuffer_size`), so the bytes searched are counted as
//! the open's work ([`Meter::count_decode`], `Limits::max_decode_work`):
//! `string_to_null` counts each search's bytes, and a blob, whose rows
//! each look two strings up in its string buffer without consuming
//! anything, finds the buffer's NULs once, on its first local string, in one
//! counted pass, and answers every offset from them by a binary search, so
//! a row costs the same whatever its strings' lengths (round 4: each row
//! searched up to 32,767 bytes for each of its strings, and nothing counted
//! it; `tests::a_blobs_rows_search_its_string_buffer_once_whatever_their_strings`).
//! A search is counted after it is made, so one the allowance refuses is
//! counted too and every later piece of the open's work is refused (round 5:
//! a refused search counted nothing, so each later one searched again), and
//! a search that finds no NUL fails without searching again.
//!
//! Owner: package C.

use std::sync::Arc;

use bytes::Bytes;

use crate::error::Error;
use crate::limits::{Limits, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::stream::{
    Endian, Reader, STRING_TO_NULL_WINDOW, py_read, py_string_to_null_found, py_string_window,
};
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
/// buffer `r` reads), the bytes its search reads (up to and including the
/// NUL, or the whole window) counted first as `meter`'s work (module docs,
/// "Work").
pub(crate) fn string_to_null(
    r: &mut Reader<'_>,
    data: &Bytes,
    meter: &Meter,
) -> Result<RawStr, Error> {
    let (len, pos) = (r.data().len(), r.position());
    let window = py_string_window(len, pos, STRING_TO_NULL_WINDOW);
    let found = r
        .data()
        .get(window.clone())
        .and_then(|w| w.iter().position(|b| *b == 0));
    let searched = found.map_or(window.len(), |k| k + 1);
    count_done(meter, u64::try_from(searched).unwrap_or(u64::MAX))?;
    let nul = found.map(|k| window.start + k);
    match py_string_to_null_found(len, pos, STRING_TO_NULL_WINDOW, nul) {
        Some((range, new)) => {
            r.set_position(new);
            Ok(shared(data, range))
        }
        // As the reader's own read raises.
        None => Err(r.string_to_null_error()),
    }
}

/// Counts `bytes` of work already done as `meter`'s: refused, as
/// [`Meter::count_decode`] refuses, when it passes the open's allowance, and
/// counted then too (the bytes were read), so every later piece of the
/// open's work is refused ([`Meter::work_used_up`]).
fn count_done(meter: &Meter, bytes: u64) -> Result<(), Error> {
    meter.count_decode(bytes).map_err(|e| {
        meter.add_work(bytes);
        Error::from(e)
    })
}

/// The NULs of a blob's string buffer, by position, found in one pass that
/// counts the buffer as the open's work and kept in a table charged to the
/// input's meter: where a search from any offset stops, in a binary search
/// (module docs, "Work").
struct NulIndex {
    nuls: MeteredVec<u32>,
}

impl NulIndex {
    fn new(buffer: &[u8], meter: &Arc<Meter>) -> Result<Self, Error> {
        meter.count_decode(u64::try_from(buffer.len()).unwrap_or(u64::MAX))?;
        let mut nuls = MeteredVec::new(meter);
        for (at, _) in buffer.iter().enumerate().filter(|(_, b)| **b == 0) {
            // A string buffer is a slice of at most `i32::MAX` bytes
            // (`stringbuffer_size`), so a position fits.
            let at = u32::try_from(at).map_err(|_| Error::Malformed {
                what: "typetree blob",
                detail: "a string buffer past 4 GiB".into(),
            })?;
            nuls.push(at)?;
        }
        Ok(Self { nuls })
    }

    /// The first NUL in `window`.
    fn first(&self, window: &std::ops::Range<usize>) -> Option<usize> {
        let nuls = self.nuls.as_slice();
        let position = |p: &u32| usize::try_from(*p).unwrap_or(usize::MAX);
        let at = nuls.partition_point(|p| position(p) < window.start);
        nuls.get(at).map(position).filter(|p| *p < window.end)
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
        let ty = string_to_null(r, data, meter)?;
        let name = string_to_null(r, data, meter)?;
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
    let mut strings = BlobStrings {
        buffer: &buffer,
        nuls: None,
        common,
        meter,
        last_unknown: None,
    };
    for row in rows_data.chunks_exact(size) {
        let b = BlobRow { row, endian };
        let version = i16::from_le_bytes(b.bytes(0));
        let level = row.get(2).copied().unwrap_or(0);
        let type_flags = row.get(3).copied().unwrap_or(0);
        // UnityPy helpers/TypeTreeNode.py:135-138: the type before the name.
        let ty = strings.get(b.u32(4))?;
        let name = strings.get(b.u32(8))?;
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

/// What `read_string` in `parse_blob` reads (`helpers/TypeTreeNode.py:122-129`):
/// the string buffer, its NULs once a local string needs them, the common
/// strings, and the last unknown common offset's decimal string.
struct BlobStrings<'a> {
    buffer: &'a Bytes,
    nuls: Option<NulIndex>,
    common: Option<&'a CommonStrings>,
    meter: &'a Arc<Meter>,
    last_unknown: Option<(u32, RawStr)>,
}

impl BlobStrings<'_> {
    /// `read_string(stringbuffer_reader, value)`: a local offset reads to a
    /// NUL in the string buffer from there (a reader over the buffer, the
    /// 32,767-byte window), the NUL found in the buffer's index; a high-bit
    /// offset is the common string (`common`'s, or
    /// [`super::commonstring::COMMON_STRINGS`]'s), or the offset's decimal
    /// string when the table has none, the last such string shared with the
    /// next row that names it.
    fn get(&mut self, value: u32) -> Result<RawStr, Error> {
        if value & 0x8000_0000 == 0 {
            let nuls = match &mut self.nuls {
                Some(nuls) => nuls,
                none => none.insert(NulIndex::new(self.buffer, self.meter)?),
            };
            let (len, pos) = (self.buffer.len(), i128::from(value));
            let window = py_string_window(len, pos, STRING_TO_NULL_WINDOW);
            let nul = nuls.first(&window);
            let (range, _) = py_string_to_null_found(len, pos, STRING_TO_NULL_WINDOW, nul)
                .ok_or_else(|| Error::Malformed {
                    what: "typetree string",
                    detail: format!("String not terminated at offset {value}").into(),
                })?;
            return Ok(shared(self.buffer, range));
        }
        common_or_decimal(value, self.common, self.meter, &mut self.last_unknown)
    }
}

/// A high-bit offset: the common string, or its decimal string
/// ([`BlobStrings::get`]).
fn common_or_decimal(
    value: u32,
    common: Option<&CommonStrings>,
    meter: &Arc<Meter>,
    last_unknown: &mut Option<(u32, RawStr)>,
) -> Result<RawStr, Error> {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;

    use super::{BlobStrings, parse_blob, string_to_null};
    use crate::budget::{Reservation, Unbounded};
    use crate::error::{Error, LimitKind};
    use crate::limits::{Limits, Meter};
    use crate::stream::{Endian, Reader, STRING_TO_NULL_WINDOW, py_string_to_null};

    /// Every local offset of a string buffer reads, through the buffer's NUL
    /// index, what a search reads (`py_string_to_null`, the scaffold's port
    /// of `read_string_to_null`, which `tests/fixtures/scaffold/reader.json`
    /// pins to UnityPy): buffers with no NUL, only NULs and NULs at random,
    /// shorter and longer than the window, at every offset to past the end.
    #[test]
    fn a_blob_string_found_through_the_index_is_the_one_a_search_finds() {
        let mut rng = unityrs_testkit::Rng::new(4);
        let meter = Meter::unlimited();
        let mut buffers: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![b'a'],
            vec![0; 100],
            vec![b'a'; 32_766],
            vec![b'a'; 32_767],
            vec![b'a'; 32_768],
            vec![b'a'; 40_000],
        ];
        let mut two = vec![b'a'; 70_000];
        two[32_766] = 0;
        two[65_534] = 0;
        buffers.push(two);
        for _ in 0..300 {
            let (len, density) = (rng.below(300), rng.below(9));
            buffers.push(
                (0..len)
                    .map(|_| if rng.below(8) < density { 0 } else { b'x' })
                    .collect(),
            );
        }
        let mut checked = 0;
        for buffer in buffers {
            let bytes = Bytes::from(buffer.clone());
            let mut strings = BlobStrings {
                buffer: &bytes,
                nuls: None,
                common: None,
                meter: &meter,
                last_unknown: None,
            };
            let len = buffer.len();
            let mut offsets: Vec<usize> = (0..len.min(300) + 3).collect();
            offsets.extend([
                len.saturating_sub(32_768),
                len.saturating_sub(32_767),
                len.saturating_sub(32_766),
                32_766,
                32_767,
                32_768,
                len + 100,
                0x7FFF_FFFF,
            ]);
            for offset in offsets {
                let value = u32::try_from(offset.min(0x7FFF_FFFF)).unwrap();
                let want = py_string_to_null(&buffer, i128::from(value), STRING_TO_NULL_WINDOW)
                    .map(|(range, _)| buffer[range].to_vec());
                let got = strings.get(value).ok().map(|s| s.as_bytes().to_vec());
                assert_eq!(got, want, "offset {value} of {len} bytes");
                checked += 1;
            }
        }
        assert!(checked > 40_000, "{checked}");
    }

    fn parse(data: &Bytes, limits: &Limits) -> (Result<usize, Error>, Arc<Meter>) {
        let meter = Meter::for_input(limits, Arc::new(Unbounded), Reservation::Input);
        let mut r = Reader::new(data, Endian::Little);
        let got = parse_blob(&mut r, data, 22, &meter, limits, None).map(|rows| {
            assert!(
                rows.as_slice()
                    .iter()
                    .all(|row| row.ty.len() == 32_767 && row.name.len() == 32_767)
            );
            rows.len()
        });
        (got, meter)
    }

    /// Round 4: each row of a blob whose type and name are at offset 0 of a
    /// 64 KiB string buffer with no NUL reads two 32,767-byte strings, and
    /// the lookup searched the window for each, 6.5 x 10^9 bytes for 100,000
    /// rows that no limit counted. Now the buffer is searched once, on the
    /// first local string, and that one pass is all the work the rows count,
    /// whatever their number or their strings' lengths; it is the open's
    /// work, so an allowance below it refuses the blob.
    #[test]
    fn a_blobs_rows_search_its_string_buffer_once_whatever_their_strings() {
        for rows in [1, 1_000, 100_000] {
            let data = Bytes::from(unityrs_testkit::blob::no_nul_blob(rows, 65_536));
            let (got, meter) = parse(&data, &Limits::default());
            assert_eq!(got.unwrap(), rows);
            assert_eq!(meter.decoded(), 65_536, "{rows} rows: the buffer, once");
        }
        let limits = Limits {
            max_decode_work: 65_535,
            ..Limits::default()
        };
        let data = Bytes::from(unityrs_testkit::blob::no_nul_blob(1_000, 65_536));
        let (got, _) = parse(&data, &limits);
        assert!(
            matches!(got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork
                && l.value == 65_536 && l.max == 65_535),
            "{got:?}"
        );
    }

    /// `read_string_to_null` counts what its search reads as the open's
    /// work: to and including the NUL, or the whole window where it finds
    /// none.
    #[test]
    fn a_string_read_counts_the_bytes_its_search_reads() {
        let mut data = vec![b'a'; 40_000];
        data[9] = 0;
        let data = Bytes::from(data);
        let meter = Meter::unlimited();
        let mut r = Reader::new(&data, Endian::Little);
        assert_eq!(string_to_null(&mut r, &data, &meter).unwrap().len(), 9);
        assert_eq!(meter.decoded(), 10);
        assert_eq!(
            string_to_null(&mut r, &data, &meter).unwrap().len(),
            STRING_TO_NULL_WINDOW
        );
        assert_eq!(meter.decoded(), 10 + 32_767);
        // Past the window's end there is no NUL and no more room: it raises,
        // having searched the rest.
        assert!(string_to_null(&mut r, &data, &meter).is_err());
        assert_eq!(meter.decoded(), 40_000);
    }

    /// Round 5's sweep: a search the open's allowance refuses was made
    /// before it was counted, so it counts too, and the open's work is used
    /// up: each later search, which would have searched its window again
    /// and been refused counting nothing, is refused before any work.
    #[test]
    fn a_search_the_allowance_refuses_is_counted_and_uses_the_work_up() {
        let data = Bytes::from(vec![b'a'; 40_000]);
        let limits = Limits {
            max_decode_work: 1_000,
            ..Limits::default()
        };
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        let mut r = Reader::new(&data, Endian::Little);
        let got = string_to_null(&mut r, &data, &meter);
        assert!(
            matches!(got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork
                && l.value == 32_767 && l.max == 1_000),
            "{got:?}"
        );
        assert_eq!(meter.decoded(), 32_767);
        assert!(meter.work_used_up().is_some());
    }
}
