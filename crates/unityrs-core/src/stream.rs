//! A bounded reader over a byte slice with the semantics of UnityPy's
//! `EndianBinaryReader_Memoryview` (`streams/EndianBinaryReader.py:230-285`).
//!
//! Container and metadata parsers read through it; the value walker has its
//! own cursor (UnityPyBoost's bounds rule differs, plan section 3.10). No
//! method panics: every read UnityPy refuses is an [`Error::OutOfBounds`].
//!
//! **Positions are signed** (plan section 3.3). A Python `Position` is an
//! unbounded integer that UnityPy moves with plain arithmetic, so it can go
//! negative (a negative `node_count` in `parse_blob`, `file_size -
//! metadata_size` below format 9, `Length - compressedSize` for blocks-info
//! at the end, a negative `TypelessData` length on the pure-Python path) or
//! past the end, and the reads after it go on with Python's rules, which this
//! module's helpers implement once and an oracle fixture pins
//! (`tests/fixtures/scaffold/reader.json`, from `tools/fixtures/scaffold.py`):
//! - [`py_read`]: `read(n)`, a Python slice (negative indexes count from the
//!   end; a reversed range is empty); the position moves by `n` whatever
//!   the slice held;
//! - [`py_unpack_from`]: every fixed-width read of 2, 4 or 8 bytes is
//!   `struct.unpack_from(view, Position)` (`:327-331`): a negative offset
//!   counts from the end, and the read must fit;
//! - one-byte reads (`read_u8`, `read_i8`, `read_bool`) are
//!   `unpack(fmt, read(1))` (`:91-95`): the slice of [`py_read`] must hold
//!   exactly one byte, and the position has moved by 1 even when it raises
//!   (at `-1`, `view[-1:0]` is empty);
//! - [`py_align`]: `align_stream`, Python's floor modulo (`-7` aligns to `-4`);
//! - [`py_string_to_null`]: `re.search` clamps its start and end into the
//!   buffer, a hit sets the absolute position after the NUL, and a miss reads
//!   `max_length` bytes as a slice unless `pos + max_length >= len`.
//!
//! Owner: package C. The free functions and [`Reader::read_i32_array`] are
//! the scaffold's (implemented).

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

use std::ops::Range;
use std::sync::Arc;

use crate::error::Error;
use crate::limits::{Meter, MeteredVec};

/// Byte order of a reader or a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endian {
    /// `<`
    Little,
    /// `>`
    Big,
}

/// `read_string_to_null`'s default `max_length` (`EndianBinaryReader.py:275`).
/// Within the window the string runs to its NUL. With no NUL in the window it
/// is cut at the window when more bytes follow, and raises otherwise.
pub const STRING_TO_NULL_WINDOW: usize = 32_767;

/// The memoryview reader's `read(size)` at `pos` over `len` bytes
/// (`streams/EndianBinaryReader.py:248-255`), with Python's arithmetic: size
/// 0 reads nothing and leaves the position; size -1 reads to the end; any
/// other size is the slice `view[pos : pos + size]`, where a negative index
/// counts from the end, indexes clamp to `0..=len` and a reversed range is
/// empty. Returns the byte range and the new position, `pos + size` (`len`
/// for -1), which may be negative or past the end, as a Python `Position` can
/// be. A bundle entry (`File.py:78-81`), `parse_blob`'s node area and string
/// buffer, and the pure-Python reader's `TypelessData` all read this way
/// (plan section 3.6). Implemented by the scaffold.
#[must_use]
pub fn py_read(len: usize, pos: i128, size: i64) -> (Range<usize>, i128) {
    if size == 0 {
        return (0..0, pos);
    }
    let l = i128::try_from(len).unwrap_or(i128::MAX);
    let size = if size == -1 {
        l.saturating_sub(pos)
    } else {
        i128::from(size)
    };
    let stop = pos.saturating_add(size);
    let index = |i: i128| -> usize {
        let i = if i < 0 { i.saturating_add(l) } else { i };
        usize::try_from(i.clamp(0, l)).unwrap_or(0)
    };
    let start = index(pos);
    let end = index(stop).max(start);
    (start..end, stop)
}

/// `struct.unpack_from(fmt, view, pos)` for a format of `n` bytes over `len`
/// bytes (`CPython`'s `s_unpack_from`): the start index of the bytes it reads,
/// or `None` where it raises `struct.error`. A negative `pos` counts from the
/// end and must leave the whole read before the end (`pos + n <= 0`) and
/// inside the buffer (`pos + len >= 0`). Every fixed-width read of 2, 4 or 8
/// bytes in UnityPy's memoryview reader takes this path, then moves the
/// position by `n` (only on success). Implemented by the scaffold.
#[must_use]
pub fn py_unpack_from(len: usize, pos: i128, n: usize) -> Option<usize> {
    let l = i128::try_from(len).ok()?;
    let n = i128::try_from(n).ok()?;
    let mut offset = pos;
    if offset < 0 {
        if offset.saturating_add(n) > 0 || offset.saturating_add(l) < 0 {
            return None;
        }
        offset = offset.saturating_add(l);
    }
    if l.saturating_sub(offset) < n {
        return None;
    }
    usize::try_from(offset).ok()
}

/// `align_stream(n)` (`streams/EndianBinaryReader.py:186-187`):
/// `pos + (n - pos % n) % n` with Python's floor modulo, so a negative
/// position aligns towards zero (`-7` to `-4` for `n = 4`). `n` 0 leaves the
/// position (Python would raise; no caller passes 0). Implemented by the scaffold.
#[must_use]
pub fn py_align(pos: i128, n: u32) -> i128 {
    let n = i128::from(n);
    if n == 0 {
        return pos;
    }
    pos.saturating_add((n - pos.rem_euclid(n)) % n)
}

/// `read_string_to_null(max_length)` of the memoryview reader
/// (`streams/EndianBinaryReader.py:275-284`) at `pos` over `data`: the
/// string's byte range and the new position, or `None` where it raises
/// ("String not terminated"). `re.search(view, pos, pos + max_length)` clamps
/// both bounds into `0..=len`, so a negative start searches from 0; a hit
/// returns the bytes up to the first NUL in the window and sets the position
/// after that NUL (an absolute index). With no NUL in the window it raises
/// when `pos + max_length >= len`, and otherwise returns `read(max_length)`,
/// a slice from `pos` that may hold a NUL or nothing at all. Implemented by
/// the scaffold.
#[must_use]
pub fn py_string_to_null(
    data: &[u8],
    pos: i128,
    max_length: usize,
) -> Option<(Range<usize>, i128)> {
    let l = i128::try_from(data.len()).ok()?;
    let max = i128::try_from(max_length).ok()?;
    let clamp = |i: i128| usize::try_from(i.clamp(0, l)).unwrap_or(0);
    let start = clamp(pos);
    let end = clamp(pos.saturating_add(max)).max(start);
    if let Some(k) = data
        .get(start..end)
        .and_then(|w| w.iter().position(|b| *b == 0))
    {
        let nul = start + k;
        return Some((start..nul, i128::try_from(nul).ok()? + 1));
    }
    if pos.saturating_add(max) >= l {
        return None;
    }
    Some(py_read(data.len(), pos, i64::try_from(max_length).ok()?))
}

/// A cursor over `data`. Positions are absolute within `data` and signed, as
/// a Python `Position` (module docs): every read follows the helper of its
/// Python primitive at whatever position it is, negative or past the end.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: i128,
    endian: Endian,
}

impl<'a> Reader<'a> {
    /// A reader at position 0.
    pub fn new(data: &'a [u8], endian: Endian) -> Self {
        Self {
            data,
            pos: 0,
            endian,
        }
    }

    /// The whole slice.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The position, as Python holds it.
    pub fn position(&self) -> i128 {
        self.pos
    }

    /// Moves the position anywhere, negative or past the end, as an
    /// assignment to `reader.Position` does.
    pub fn set_position(&mut self, pos: i128) {
        self.pos = pos;
    }

    /// The byte order of multi-byte reads.
    pub fn endian(&self) -> Endian {
        self.endian
    }

    /// Changes the byte order (a `SerializedFile` header switches it).
    pub fn set_endian(&mut self, endian: Endian) {
        self.endian = endian;
    }

    /// The bytes `read(-1)` would return from here: what a table's growth may
    /// count on (plan section 3.3).
    pub fn remaining(&self) -> usize {
        py_read(self.data.len(), self.pos, -1).0.len()
    }

    /// `align_stream(n)`: [`py_align`], without a bounds check.
    pub fn align(&mut self, n: u32) {
        todo!()
    }

    /// `n` bytes where UnityPy's caller unpacks them whole (`read(n)`, then
    /// `struct.unpack`): the [`py_read`] slice must hold `n` bytes, else
    /// `OutOfBounds`; the position moves by `n` either way.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        todo!()
    }

    /// `read(n)` as it is: the [`py_read`] slice, short or empty at the ends
    /// (`read_bytes` on a truncated block, `File.py:79`), the position moved
    /// by `n`.
    pub fn read_bytes_short(&mut self, n: i64) -> &'a [u8] {
        todo!()
    }

    /// `read_string_to_null` with the 32,767-byte window: [`py_string_to_null`].
    pub fn read_string_to_null(&mut self) -> Result<&'a [u8], Error> {
        todo!()
    }

    /// One byte as a bool (non-zero is true): `unpack("?", read(1))`.
    pub fn read_bool(&mut self) -> Result<bool, Error> {
        todo!()
    }

    /// `u8`: `unpack("B", read(1))`.
    pub fn read_u8(&mut self) -> Result<u8, Error> {
        todo!()
    }

    /// `i8`: `unpack("b", read(1))`.
    pub fn read_i8(&mut self) -> Result<i8, Error> {
        todo!()
    }

    /// `u16` in the reader's byte order: [`py_unpack_from`].
    pub fn read_u16(&mut self) -> Result<u16, Error> {
        todo!()
    }

    /// `i16` in the reader's byte order: [`py_unpack_from`].
    pub fn read_i16(&mut self) -> Result<i16, Error> {
        todo!()
    }

    /// `u32` in the reader's byte order: [`py_unpack_from`].
    pub fn read_u32(&mut self) -> Result<u32, Error> {
        todo!()
    }

    /// `i32` in the reader's byte order: [`py_unpack_from`].
    pub fn read_i32(&mut self) -> Result<i32, Error> {
        todo!()
    }

    /// `u64` in the reader's byte order: [`py_unpack_from`].
    pub fn read_u64(&mut self) -> Result<u64, Error> {
        todo!()
    }

    /// `i64` in the reader's byte order: [`py_unpack_from`].
    pub fn read_i64(&mut self) -> Result<i64, Error> {
        todo!()
    }

    /// A fixed-size byte array (GUIDs, hashes, reserved bytes): `read(N)`,
    /// which must hold `N` bytes where UnityPy's caller needs them all.
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        todo!()
    }

    /// `read_int_array` (`EndianBinaryReader.py:182`, `read_array_struct` at
    /// `:258-264`): an `i32` count read as [`py_unpack_from`] reads it (the
    /// position moves by 4), then `struct.unpack_from` of that many `i32` at
    /// the new position (a negative one counts from the end), the position
    /// moving by `4 * count`. A negative count raises (`Struct("<-1i")` is a
    /// bad format) with the position after the count, as does an array the
    /// bytes do not hold. The array is a vector on `meter`, the input's,
    /// charged after the bytes are checked and before it is allocated, so a
    /// count allocates nothing the bytes do not back and the array counts
    /// against its input while it is kept (round 5: this returned a plain
    /// `Vec`, the one input-sized allocation a parser made uncharged); a
    /// refused charge is the meter's or the budget's error, with the
    /// position after the count. Implemented by the scaffold; the cases are
    /// recorded from UnityPy 1.25.2 (`read_i32_array_reads_as_unitypy_and_charges_the_meter`).
    pub fn read_i32_array(&mut self, meter: &Arc<Meter>) -> Result<MeteredVec<i32>, Error> {
        const WHAT: &str = "int array";
        let len = self.data.len();
        let at = py_unpack_from(len, self.pos, 4).ok_or(Error::OutOfBounds {
            what: WHAT,
            pos: self.pos,
            need: 4,
            have: u64::try_from(self.remaining()).unwrap_or(u64::MAX),
        })?;
        let count = self.i32_at(at);
        self.pos = self.pos.saturating_add(4);
        let Ok(n) = usize::try_from(count) else {
            return Err(Error::Malformed {
                what: WHAT,
                detail: format!("negative count {count}").into(),
            });
        };
        let bytes = n.checked_mul(4);
        let at =
            bytes
                .and_then(|b| py_unpack_from(len, self.pos, b))
                .ok_or(Error::OutOfBounds {
                    what: WHAT,
                    pos: self.pos,
                    need: u64::try_from(n).unwrap_or(u64::MAX).saturating_mul(4),
                    have: u64::try_from(self.remaining()).unwrap_or(u64::MAX),
                })?;
        let mut out = MeteredVec::with_capacity(meter, n)?;
        for i in 0..n {
            out.push(self.i32_at(at + 4 * i))?;
        }
        self.pos = self.pos.saturating_add(i128::from(count) * 4);
        Ok(out)
    }

    /// The `i32` at `at`, which [`py_unpack_from`] checked, in the reader's
    /// byte order.
    fn i32_at(&self, at: usize) -> i32 {
        let mut b = [0u8; 4];
        if let Some(src) = self.data.get(at..at + 4) {
            b.copy_from_slice(src);
        }
        match self.endian {
            Endian::Little => i32::from_le_bytes(b),
            Endian::Big => i32::from_be_bytes(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Endian, Reader, py_read};
    use crate::budget::{Reservation, Unbounded};
    use crate::error::Error;
    use crate::error::LimitKind;
    use crate::limits::{Meter, heap_cost};

    #[test]
    fn read_i32_array_reads_as_unitypy_and_charges_the_meter() {
        // Recorded from UnityPy 1.25.2's memoryview reader (`read_int_array`
        // at a set `Position`): (bytes, endian, position) -> values and the
        // new position, or the error and the position after it.
        let le = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let be = |v: &[i32]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        let m = Meter::unlimited();
        let ok = |data: &[u8], endian, pos: i128| {
            let mut r = Reader::new(data, endian);
            r.set_position(pos);
            let v = r.read_i32_array(&m).unwrap();
            (v.as_slice().to_vec(), r.position(), v.charge().bytes())
        };
        assert_eq!(
            ok(&le(&[3, 1, -2, 3]), Endian::Little, 0),
            (vec![1, -2, 3], 16, heap_cost(12))
        );
        assert_eq!(
            ok(&be(&[3, 1, -2, 3]), Endian::Big, 0),
            (vec![1, -2, 3], 16, heap_cost(12))
        );
        // A negative position counts from the end, for the count and the
        // array alike.
        assert_eq!(
            ok(&le(&[1, 7, 9]), Endian::Little, -12),
            (vec![7], -4, heap_cost(4))
        );
        assert_eq!(ok(&le(&[0]), Endian::Little, 0), (vec![], 4, 0));

        let err = |data: &[u8], limit: u64| {
            let m = Meter::new(limit, std::sync::Arc::new(Unbounded), Reservation::Input);
            let mut r = Reader::new(data, Endian::Little);
            let e = r.read_i32_array(&m).unwrap_err();
            (e, r.position(), m.used())
        };
        // UnityPy: `bad char in struct format`, Position 4.
        let (e, pos, used) = err(&le(&[-1, 5]), u64::MAX);
        assert!(matches!(e, Error::Malformed { .. }), "{e:?}");
        assert_eq!((pos, used), (4, 0));
        // A huge count over 4 bytes: refused by the bytes, nothing charged.
        let (e, pos, used) = err(&le(&[i32::MAX, 5]), u64::MAX);
        assert!(
            matches!(
                e,
                Error::OutOfBounds {
                    need: 8_589_934_588,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!((pos, used), (4, 0));
        let (e, pos, _) = err(&[1, 0], u64::MAX);
        assert!(matches!(e, Error::OutOfBounds { need: 4, .. }), "{e:?}");
        assert_eq!(pos, 0);
        // An array the bytes hold but the meter does not: refused before it
        // is allocated.
        let (e, pos, used) = err(&le(&[64; 65]), 100);
        assert!(
            matches!(&e, Error::Limit(l) if l.kind == LimitKind::InputMemory),
            "{e:?}"
        );
        assert_eq!((pos, used), (4, 0));
    }

    #[test]
    fn py_read_is_a_python_slice() {
        // Recorded from UnityPy 1.25.2's memoryview reader over bytes(range(10)):
        // (position, size) -> (bytes read, new position).
        let cases: &[(i128, i64, std::ops::Range<usize>, i128)] = &[
            (0, 4, 0..4, 4),
            (8, 4, 8..10, 12),
            (12, 4, 10..10, 16),
            (3, 0, 0..0, 3),
            (3, -1, 3..10, 10),
            (-3, 2, 7..9, -1),
            (-3, 5, 7..7, 2),
            (5, -3, 5..5, 2),
            (2, -5, 2..7, -3),
            (-20, 5, 0..0, -15),
            (-1, -1, 9..10, 10),
            // Past the end, size -1: `size = Length - Position` is -2, so the
            // position comes back to the end (UnityPy: `read(-1)` at 12 of 10
            // leaves Position 10).
            (12, -1, 10..10, 10),
            (15, -1, 10..10, 10),
            (i128::from(i64::MIN), i64::MAX, 0..9, -1),
        ];
        for (pos, size, range, new_pos) in cases {
            let (r, p) = py_read(10, *pos, *size);
            assert_eq!(r.len(), range.len(), "({pos}, {size})");
            if !range.is_empty() {
                assert_eq!(r, *range, "({pos}, {size})");
            }
            assert_eq!(p, *new_pos, "({pos}, {size})");
        }
    }
}
