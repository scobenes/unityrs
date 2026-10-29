//! A bounded reader over a byte slice with the semantics of UnityPy's
//! `EndianBinaryReader_Memoryview` (`streams/EndianBinaryReader.py:230-285`).
//!
//! Container and metadata parsers read through it; the value walker has its
//! own cursor (UnityPyBoost's bounds rule differs, plan section 3.10). No
//! method panics: every read past the end is an [`Error::OutOfBounds`].
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

use std::ops::Range;

use crate::error::Error;

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

/// A cursor over `data`. Positions are absolute within `data`.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
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

    /// The position.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Moves the position. A position past the end is allowed, as in Python;
    /// the next read fails.
    pub fn set_position(&mut self, pos: usize) {
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

    /// Bytes from the position to the end (0 when past the end).
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Rounds the position up to a multiple of `n`, without a bounds check.
    pub fn align(&mut self, n: usize) {
        todo!()
    }

    /// `n` bytes, or `OutOfBounds`.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        todo!()
    }

    /// Up to `n` bytes: a memoryview slice, short at the end (`read_bytes` on a
    /// truncated block, `File.py:79`).
    pub fn read_bytes_short(&mut self, n: usize) -> &'a [u8] {
        todo!()
    }

    /// `read_string_to_null` with the 32,767-byte window. The NUL is consumed;
    /// a string cut at the window leaves the position after the cut.
    pub fn read_string_to_null(&mut self) -> Result<&'a [u8], Error> {
        todo!()
    }

    /// One byte as a bool (non-zero is true).
    pub fn read_bool(&mut self) -> Result<bool, Error> {
        todo!()
    }

    /// `u8`.
    pub fn read_u8(&mut self) -> Result<u8, Error> {
        todo!()
    }

    /// `i8`.
    pub fn read_i8(&mut self) -> Result<i8, Error> {
        todo!()
    }

    /// `u16` in the reader's byte order.
    pub fn read_u16(&mut self) -> Result<u16, Error> {
        todo!()
    }

    /// `i16` in the reader's byte order.
    pub fn read_i16(&mut self) -> Result<i16, Error> {
        todo!()
    }

    /// `u32` in the reader's byte order.
    pub fn read_u32(&mut self) -> Result<u32, Error> {
        todo!()
    }

    /// `i32` in the reader's byte order.
    pub fn read_i32(&mut self) -> Result<i32, Error> {
        todo!()
    }

    /// `u64` in the reader's byte order.
    pub fn read_u64(&mut self) -> Result<u64, Error> {
        todo!()
    }

    /// `i64` in the reader's byte order.
    pub fn read_i64(&mut self) -> Result<i64, Error> {
        todo!()
    }

    /// A fixed-size byte array (GUIDs, hashes, reserved bytes).
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        todo!()
    }

    /// `read_int_array`: an `i32` count, then that many `i32`. A negative count
    /// or a short buffer raises, as `struct.unpack` does.
    pub fn read_i32_array(&mut self) -> Result<Vec<i32>, Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::py_read;

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
