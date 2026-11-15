// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.
//! Dispatch object reads by stored size, stream kind and serialized-file tier.

use super::{ParseError, ReadCtx, boost, python, stream_refused};
use crate::stream::ReaderKind;
use crate::typetree::Tree;
use crate::{Bytes, Parsed};
use std::sync::Arc;

/// `TypeTreeHelper.read_typetree` over bytes (UnityPy's memory reader):
/// [`boost::read`] over the Python slice `file[byte_start : byte_start +
/// byte_size]` ([`crate::stream::py_read`]: short at the end of a truncated
/// file, from the end for a negative start) when `byte_size > 0`, else
/// [`python::read`] from `byte_start`; then, with `check_read`, `bytes_read
/// != byte_size` is [`ParseError::CheckRead`] carrying the value.
pub fn read_typetree(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
) -> Result<Parsed, ParseError> {
    read_typetree_with_reader(
        file,
        byte_start,
        byte_size,
        tree,
        ctx,
        check_read,
        ReaderKind::Memory,
    )
}

/// [`read_typetree`] from the file's reader of `kind`
/// ([`crate::files::SerializedFile::reader_kind`]): `reset()` sets the
/// position to `byte_start`, which the stream reader refuses when negative
/// or at or above 2^63 ([`super::ReadDetail::Stream`], kind `other`), as UnityPy's
/// does on every machine; a `byte_start` past the largest file the file
/// system takes is UnityPy's `OSError` on that machine, and taken here
/// ([`crate::stream::ReaderKind`], `PARITY.md` section 19.3). The Boost
/// path is given `read_bytes(byte_size)` from there (on both readers the
/// bytes that exist), and the pure-Python reader reads by the reader's
/// rules ([`python::read_with_reader`]).
pub fn read_typetree_with_reader(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
    kind: ReaderKind,
) -> Result<Parsed, ParseError> {
    read_typetree_versioned(
        file,
        byte_start,
        byte_size,
        tree,
        ctx,
        check_read,
        (kind, false),
    )
}

/// Stored HEAD-tier trees use HEAD's pure-Python array behavior.
pub(crate) fn read_typetree_versioned(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
    mode: (ReaderKind, bool),
) -> Result<Parsed, ParseError> {
    let (kind, head) = mode;
    // UnityPy files/ObjectReader.py:214-215, :251: `reset()`.
    let start = kind.seek(byte_start).map_err(|e| stream_refused(0, e))?;
    // UnityPy TypeTreeHelper.py:146-153: `byte_size and read_typetree_boost`.
    let parsed = if byte_size > 0 {
        // UnityPy TypeTreeHelper.py:147: `reader.read_bytes(byte_size)`: on
        // the memory reader a Python slice of the file from `byte_start`.
        let (range, _) = kind
            .read(file.len(), start, i64::from(byte_size))
            .map_err(|e| stream_refused(0, e))?;
        boost::read(&file.slice(range), tree, ctx)?
    } else {
        python::read_versioned(file, start, tree, ctx, kind, head)?
    };
    // UnityPy TypeTreeHelper.py:155-156.
    if check_read && parsed.bytes_read != i64::from(byte_size) {
        return Err(ParseError::CheckRead {
            expected: byte_size,
            parsed: Arc::new(parsed),
        });
    }
    Ok(parsed)
}
