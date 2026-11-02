// Ported from UnityPy 1.25.2 (MIT): UnityPy/streams/EndianBinaryReader.py. See THIRD_PARTY_LICENSES.
//! Which of UnityPy's two readers a file is read with, and the stream
//! reader's rules (`EndianBinaryReader_Streamable`,
//! `streams/EndianBinaryReader.py:287-324`).
//!
//! `EndianBinaryReader(item)` picks its class by the input's type (`:40-63`):
//! bytes get the memory reader, a file object the stream reader. UnityPy's
//! `Environment` opens a path as a file (`environment.py:136-139`, the
//! buffered file `fs.open(path, "rb")` gives; a folder's files through the
//! builtin `open`, `:75-77`), so a file loaded from a path is read by the
//! stream reader: its header, metadata and objects, the pure-Python typetree
//! reader's reads, and the slice the Boost reader is given
//! (`helpers/TypeTreeHelper.py:146-153`), and, for a reader kept as it is
//! (a `.resS`), the resource reads. What UnityPy reads from memory stays on
//! the memory reader whatever the source: a bundle's entries (its blocks are
//! decoded into bytes, `files/BundleFile.py:173-186`), a blob's string
//! buffer (`helpers/TypeTreeNode.py:118`) and a bundle's blocks-info.
//! `EndianBinaryReader_Streamable_LocalFile` (`:327-332`), the reader of a
//! `str`, is reached by no load: `Environment` opens a path before it makes
//! a reader.
//!
//! The stream reader's `read` is the file's own (`:295`), its `Position` the
//! file's `tell()` (`:298-299`), and its fixed-width reads
//! `struct.unpack(stream.read(n))` (`:345-346`). Over a file of `len` bytes,
//! recorded from UnityPy 1.25.2 on a file `fs.open` opened
//! (`tests/fixtures/scaffold/reader.json`: the `stream_` cases on a file of
//! the scratch folder, the `stream_tmpfs_` cases on a tmpfs file, whose
//! largest is 2^63 - 1):
//! - `Position = p` refuses a negative `p` (`ValueError` "Position cannot be
//!   negative", `:302-305`) and one at or above 2^63, which the file's
//!   `seek` cannot take on any machine (`ValueError` "cannot fit 'int' into
//!   an offset-sized integer", `CPython`'s `PyNumber_AsOff_t`); any other moves
//!   there, past the end included, where the file system takes it. Past the
//!   largest file a file system takes (ext4's is 2^44 - 4096 with 4 KiB
//!   blocks; tmpfs's 2^63 - 1, the `stream_tmpfs_` cases) `seek` raises
//!   `OSError` (`EINVAL`), which depends on the machine: this module does not
//!   model it, and moves there (`PARITY.md` section 19.3). A stream position
//!   is never negative.
//! - `read(n)`: `n` below -1 raises (`ValueError` "read length must be
//!   non-negative or -1"); -1 reads to the end and 0 nothing; any other `n`
//!   reads the bytes that exist, and the position moves by the bytes read,
//!   so it stops at the end, and stays where it is past the end. A buffered
//!   file allocates `n` bytes before it reads
//!   (`_bufferedreader_read_generic`), so `n` at or above 2^63 - 33 raises
//!   `OverflowError` ("byte string is too large", from `_PyBytes_FromSize`)
//!   on any machine; an `n` below that but above what the machine can
//!   allocate raises `MemoryError`, which depends on the machine: this
//!   module does not model it, and reads what exists (`PARITY.md` section
//!   19.3). The fixture's sizes are those that raise on every machine or
//!   that every machine allocates. At a position where the buffered file's
//!   fill would end past 2^63 - 1 a read raises `OSError` (`EINVAL`) on
//!   Linux (the `stream_tmpfs_read` cases at 2^63 - 1); this module reads
//!   nothing there, as past the end anywhere.
//! - a fixed-width read is `read(n)`, which raises unless it got `n` bytes,
//!   the position moved by what it got either way.
//! - `read_string_to_null(max)` is the base class's loop (`:136-144`): it
//!   reads a byte at a time until a NUL, `max` bytes, or the end, and drops
//!   the last byte it read when that byte is no NUL. So with no NUL in reach
//!   it returns one byte fewer than it read; at the end, or with `max` 0
//!   anywhere, it returns `""`; and only past the end with `max` above 0,
//!   where its first read is empty, it raises (`ValueError` "Unterminated
//!   string: [b'']").
//! - `align_stream` is the memory reader's arithmetic (`:155-156`), the
//!   position never negative, assigned through the `Position` setter: so a
//!   position it takes to 2^63 or more raises as that `seek` does
//!   ([`ReaderKind::align`]).
//!
//! The memory reader's rules are the free functions of [`super`].

use std::ops::Range;

use super::{py_align, py_read, py_string_to_null_found, py_string_window, py_unpack_from};

/// Which of UnityPy's readers a file is read with (module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReaderKind {
    /// `EndianBinaryReader_Memoryview`: bytes. What UnityPy is handed as
    /// bytes ([`crate::env::Source::Bytes`] and [`crate::env::Source::Fetch`]),
    /// and every entry of a container.
    #[default]
    Memory,
    /// `EndianBinaryReader_Streamable` over the buffered file `Environment`
    /// opens for a path: a top-level [`crate::env::Source::Path`].
    Stream,
}

/// An exception the stream reader raises where the memory reader reads on
/// (module docs). Its text is UnityPy's message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum StreamRefusal {
    /// `Position = p` with `p < 0` (`ValueError`).
    #[error("Position cannot be negative")]
    NegativePosition,
    /// `Position = p` with `p >= 2^63`: the file's `seek` takes a C offset
    /// (`ValueError`).
    #[error("cannot fit 'int' into an offset-sized integer")]
    PositionTooLarge,
    /// `read(n)` with `n < -1` (`ValueError`).
    #[error("read length must be non-negative or -1")]
    NegativeRead,
    /// `read(n)` with `2^63 - 33 <= n < 2^63`: the bytes object `n` needs
    /// (`OverflowError`).
    #[error("byte string is too large")]
    ReadTooLarge,
    /// `read(n)` with `n >= 2^63`: a C size (`OverflowError`).
    #[error("cannot fit 'int' into an index-sized integer")]
    ReadSizeOverflow,
}

impl StreamRefusal {
    /// UnityPy's exception class.
    #[must_use]
    pub fn exception(self) -> &'static str {
        match self {
            Self::NegativePosition | Self::PositionTooLarge | Self::NegativeRead => "ValueError",
            Self::ReadTooLarge | Self::ReadSizeOverflow => "OverflowError",
        }
    }

    /// The message, as UnityPy's exception carries it.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NegativePosition => "Position cannot be negative",
            Self::PositionTooLarge => "cannot fit 'int' into an offset-sized integer",
            Self::NegativeRead => "read length must be non-negative or -1",
            Self::ReadTooLarge => "byte string is too large",
            Self::ReadSizeOverflow => "cannot fit 'int' into an index-sized integer",
        }
    }
}

impl From<StreamRefusal> for crate::error::Error {
    fn from(e: StreamRefusal) -> Self {
        Self::Malformed {
            what: STREAM_READER,
            detail: e.message().into(),
        }
    }
}

const _: () = {
    crate::send_sync::<ReaderKind>();
    crate::send_sync::<StreamRefusal>();
};

/// The `what` of an [`crate::Error::Malformed`] a [`StreamRefusal`] becomes.
pub const STREAM_READER: &str = "stream reader";

/// The smallest `read(n)` a buffered file refuses for its bytes object on
/// every machine (`_PyBytes_FromSize` refuses a size above
/// `PY_SSIZE_T_MAX - PyBytesObject_SIZE`, `PyBytesObject_SIZE` being 33:
/// the fixture records `2^63 - 33` as "byte string is too large"). Below
/// it the read allocates first, so its outcome depends on the machine
/// (module docs).
const READ_TOO_LARGE: i64 = i64::MAX - 32;

impl ReaderKind {
    /// `Position = pos`: the position it leaves, or the stream reader's
    /// refusal of a negative one or one at or above 2^63.
    pub fn seek(self, pos: i128) -> Result<i128, StreamRefusal> {
        match self {
            Self::Stream if pos < 0 => Err(StreamRefusal::NegativePosition),
            Self::Stream if pos > i128::from(i64::MAX) => Err(StreamRefusal::PositionTooLarge),
            Self::Memory | Self::Stream => Ok(pos),
        }
    }

    /// `align_stream(n)` at `pos`: `Position += (n - Position % n) % n`
    /// ([`py_align`]), an assignment through the `Position` setter
    /// (`streams/EndianBinaryReader.py:155-156`), so the stream reader
    /// refuses a result its `seek` cannot take, as [`Self::seek`] does: at
    /// 2^63 or more (from a position within `n - 1` of 2^63 - 1).
    pub fn align(self, pos: i128, n: u32) -> Result<i128, StreamRefusal> {
        self.seek(py_align(pos, n))
    }

    /// `read(size)` at `pos` over `len` bytes: the bytes' range and the new
    /// position. The memory reader's is [`py_read`]; the stream reader's
    /// reads what exists from `pos` (never negative on a stream, as
    /// [`Self::seek`] refuses that) and moves by what it read.
    pub fn read(
        self,
        len: usize,
        pos: i128,
        size: i64,
    ) -> Result<(Range<usize>, i128), StreamRefusal> {
        match self {
            Self::Memory => Ok(py_read(len, pos, size)),
            Self::Stream => stream_read(len, pos, size),
        }
    }

    /// `read(size)` for a size a caller holds as a `u64` (a resource's): on
    /// the memory reader a size past `i64::MAX` slices as `i64::MAX` does,
    /// and the stream reader refuses it.
    pub fn read_u64(
        self,
        len: usize,
        pos: i128,
        size: u64,
    ) -> Result<(Range<usize>, i128), StreamRefusal> {
        match (i64::try_from(size), self) {
            (Ok(size), _) => self.read(len, pos, size),
            (Err(_), Self::Memory) => Ok(py_read(len, pos, i64::MAX)),
            (Err(_), Self::Stream) => Err(StreamRefusal::ReadSizeOverflow),
        }
    }

    /// The reader's own fixed-width read of `n` bytes (`reader.read_int()`
    /// and the like) at `pos` over `len` bytes: the start of its `n` bytes,
    /// or `None` where it raises, and the position after it. The memory
    /// reader's is `struct.unpack_from(view, Position)` ([`py_unpack_from`]),
    /// which moves the position by `n` on success only; the stream reader's
    /// is `struct.unpack(stream.read(n))`, which moves it by what it read.
    #[must_use]
    pub fn unpack(self, len: usize, pos: i128, n: usize) -> (Option<usize>, i128) {
        let step = i128::try_from(n).unwrap_or(i128::MAX);
        match self {
            Self::Memory => match py_unpack_from(len, pos, n) {
                Some(at) => (Some(at), pos.saturating_add(step)),
                None => (None, pos),
            },
            Self::Stream => {
                let size = i64::try_from(n).unwrap_or(i64::MAX);
                match stream_read(len, pos, size) {
                    Ok((range, next)) if range.len() == n => (Some(range.start), next),
                    Ok((_, next)) => (None, next),
                    Err(_) => (None, pos),
                }
            }
        }
    }

    /// `read_string_to_null(max_length)` at `pos` over `data`: the string's
    /// range and the new position, or `None` where it raises (module docs:
    /// the stream reader raises only past the end with `max_length` above
    /// 0).
    #[must_use]
    pub fn string_to_null(
        self,
        data: &[u8],
        pos: i128,
        max_length: usize,
    ) -> Option<(Range<usize>, i128)> {
        let window = py_string_window(data.len(), pos, max_length);
        let nul = data
            .get(window.clone())
            .and_then(|w| w.iter().position(|b| *b == 0))
            .map(|k| window.start + k);
        self.string_to_null_found(data.len(), pos, max_length, nul)
    }

    /// [`Self::string_to_null`]'s answer once its search of
    /// [`py_string_window`]'s window is done: `nul` is the window's first
    /// NUL, as an index of the whole buffer. The stream reader's loop reads
    /// the same window (from a position that is never negative), so one
    /// search serves both readers.
    #[must_use]
    pub(crate) fn string_to_null_found(
        self,
        len: usize,
        pos: i128,
        max_length: usize,
        nul: Option<usize>,
    ) -> Option<(Range<usize>, i128)> {
        match self {
            Self::Memory => py_string_to_null_found(len, pos, max_length, nul),
            Self::Stream => stream_string_to_null(len, pos, max_length, nul),
        }
    }
}

/// The stream reader's `read(size)` (module docs).
fn stream_read(len: usize, pos: i128, size: i64) -> Result<(Range<usize>, i128), StreamRefusal> {
    if size < -1 {
        return Err(StreamRefusal::NegativeRead);
    }
    if size >= READ_TOO_LARGE {
        return Err(StreamRefusal::ReadTooLarge);
    }
    let l = i128::try_from(len).unwrap_or(i128::MAX);
    let start = pos.clamp(0, l);
    if size == 0 || pos >= l {
        // Nothing to read: the position stays, past the end included.
        let at = usize::try_from(start).unwrap_or(len);
        return Ok((at..at, pos));
    }
    let end = if size == -1 {
        l
    } else {
        start.saturating_add(i128::from(size)).min(l)
    };
    let range = usize::try_from(start).unwrap_or(len)..usize::try_from(end).unwrap_or(len);
    Ok((range, end))
}

/// The stream reader's `read_string_to_null` (`:136-144`, module docs):
/// `nul` is the first NUL in the window `pos..min(pos + max_length, len)`.
fn stream_string_to_null(
    len: usize,
    pos: i128,
    max_length: usize,
    nul: Option<usize>,
) -> Option<(Range<usize>, i128)> {
    let l = i128::try_from(len).ok()?;
    let start = usize::try_from(pos.clamp(0, l)).ok()?;
    if max_length == 0 || pos == l {
        // The loop never runs (`len(ret) < max_length` or `Position !=
        // Length` is false before its first read): `""`, nothing read,
        // wherever the position is.
        return Some((start..start, pos));
    }
    if pos > l {
        // The loop's first `read(1)` is empty: "Unterminated string: [b'']".
        return None;
    }
    if let Some(nul) = nul {
        // The NUL is read and dropped.
        return Some((start..nul, i128::try_from(nul).ok()? + 1));
    }
    // No NUL in reach: the bytes read, less the last.
    let read = usize::try_from(l - pos).ok()?.min(max_length);
    let end = start.checked_add(read)?;
    Some((start..end - 1, i128::try_from(end).ok()?))
}

#[cfg(test)]
mod tests {
    use super::{ReaderKind, StreamRefusal};

    #[test]
    fn the_memory_kind_is_the_free_functions() {
        // `ReaderKind::Memory` delegates: a negative position and a
        // negative size read as Python slices, and nothing is refused.
        let m = ReaderKind::Memory;
        assert_eq!(m.seek(-5), Ok(-5));
        assert_eq!(m.read(10, 3, -2), Ok((3..3, 1)));
        assert_eq!(m.unpack(10, -4, 4), (Some(6), 0));
        assert_eq!(
            m.read_u64(10, 2, u64::MAX),
            Ok((2..10, 2 + i128::from(i64::MAX)))
        );
    }

    #[test]
    fn the_stream_kind_refuses_what_a_buffered_file_refuses() {
        // What CPython refuses on every machine, each edge recorded from
        // UnityPy 1.25.2 (`tests/fixtures/scaffold/reader.json`'s `stream_`
        // and `stream_tmpfs_` cases, `tests/fixtures/d/paths.json`'s
        // `resource-reads`), which `tests/reader.rs` and `tests/env_paths.rs`
        // replay in full.
        let s = ReaderKind::Stream;
        assert_eq!(s.seek(-1), Err(StreamRefusal::NegativePosition));
        assert_eq!(
            s.seek(i128::from(i64::MAX) + 1),
            Err(StreamRefusal::PositionTooLarge)
        );
        assert_eq!(s.read(10, 3, -2), Err(StreamRefusal::NegativeRead));
        assert_eq!(
            s.read(10, 3, i64::MAX - 32),
            Err(StreamRefusal::ReadTooLarge)
        );
        assert_eq!(s.read(10, 20, i64::MAX), Err(StreamRefusal::ReadTooLarge));
        assert_eq!(
            s.read_u64(10, 0, 1 << 63),
            Err(StreamRefusal::ReadSizeOverflow)
        );
        // `align_stream` assigns `Position`, so an alignment to 2^63 is the
        // seek's refusal; the memory reader's is plain arithmetic.
        let near = i128::from(i64::MAX) - 2;
        assert_eq!(s.align(near, 4), Err(StreamRefusal::PositionTooLarge));
        assert_eq!(s.align(near - 1, 4), Ok(near - 1));
        assert_eq!(ReaderKind::Memory.align(near, 4), Ok(near + 3));
        // Past the end a read is empty and the position stays.
        assert_eq!(s.read(10, 15, -1), Ok((10..10, 15)));
        assert_eq!(s.unpack(10, 8, 4), (None, 10));
        assert_eq!(s.unpack(10, 12, 0), (Some(10), 12));
        // With `max_length` 0 the string loop never runs, past the end too.
        assert_eq!(s.string_to_null(b"abc", 7, 0), Some((3..3, 7)));
        assert_eq!(s.string_to_null(b"abc", 7, 1), None);
    }

    #[test]
    fn the_stream_kind_reads_on_where_the_machine_decides() {
        // Not UnityPy's outcome on every machine: unityrs's model of the
        // edges `PARITY.md` section 19.3 admits ("a path's stream past the
        // machine's limits"), where it reads what exists.
        let s = ReaderKind::Stream;
        // A seek past the file system's largest file raises `OSError`
        // (`EINVAL`) in UnityPy; on tmpfs, whose largest is 2^63 - 1, it
        // moves there (`reader.json`'s `stream_tmpfs_set_position`).
        assert_eq!(s.seek(i128::from(i64::MAX)), Ok(i128::from(i64::MAX)));
        // A read the machine cannot allocate raises `MemoryError` in UnityPy
        // (not recorded: a buffered file allocates `n` bytes first).
        assert_eq!(s.read(10, 3, i64::MAX - 33), Ok((3..10, 10)));
        // A read of 4 GiB reads what exists where the machine allocates it,
        // as the oracle's did (`paths.json`'s `resource-reads`).
        assert_eq!(s.read_u64(10, 5, 1 << 32), Ok((5..10, 10)));
    }
}
