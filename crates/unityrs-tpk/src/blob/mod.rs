// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `TpkTypeTreeBlob` (`helpers/Tpk.py:215-240`): the decoded payload's
//! tables, read in UnityPy's order, and the helpers every record reads with
//! (`:438-469`). Each table is a [`MeteredVec`] grown as its records are
//! read, its growth charged to the provider's meter first; each count is
//! read as `Tpk.py` reads it and refused above [`TpkProvider::MAX_RECORDS`]
//! before any of its records (crate docs, "Limits").

mod classes;
mod nodes;
mod strings;

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::{RawStr, UnityVersion};

pub(crate) use classes::{ClassEntry, ClassInformation};
pub(crate) use nodes::NodeBuffer;

use crate::{TpkError, TpkProvider};

/// A type-tree blob as UnityPy's `TpkTypeTreeBlob(stream)` reads it
/// (`Tpk.py:232-240`); what it reads and nothing uses (the creation time,
/// the common strings' version information, a node's type flags, a class's
/// name, base, flags and editor root) is read and not kept.
pub(crate) struct TypeTreeBlob {
    /// `Versions` (`read_versions`, `:467-469`), each the integer UnityPy
    /// keeps (`UnityVersion(x)`), whatever its type byte.
    pub(crate) versions: MeteredVec<UnityVersion>,
    /// `ClassInformation` (`:237`).
    pub(crate) classes: ClassInformation,
    /// `CommonString.StringBufferIndices` (`:421-423`).
    pub(crate) common_indices: MeteredVec<u16>,
    /// `NodeBuffer` (`:239`).
    pub(crate) nodes: NodeBuffer,
    /// `StringBuffer` (`:240`), each string charged and shared by the rows
    /// and the common strings that name it.
    pub(crate) strings: MeteredVec<RawStr>,
}

impl TypeTreeBlob {
    /// `TpkTypeTreeBlob(BytesIO(payload))` (`Tpk.py:232-240`), every table
    /// charged to `meter`. Bytes after the string buffer are not read, as
    /// UnityPy does not read them.
    pub(crate) fn read(payload: &[u8], meter: &Arc<Meter>) -> Result<Self, TpkError> {
        let s = &mut Stream { rest: payload };
        // `(self.CreationTime,) = INT64.unpack(...)`.
        s.i64("the creation time")?;
        // `(versionCount,) = INT32.unpack(...)`, then `read_versions`.
        let count = s.i32("the version count")?;
        let versions = read_versions(s, count, meter)?;
        let classes = ClassInformation::read(s, meter)?;
        let common_indices = strings::read_common_string(s, meter)?;
        let nodes = NodeBuffer::read(s, meter)?;
        let strings = strings::read_string_buffer(s, meter)?;
        Ok(Self {
            versions,
            classes,
            common_indices,
            nodes,
            strings,
        })
    }
}

/// `read_versions(stream, count)` (`Tpk.py:467-469`): `Struct(f"<{count}Q")`,
/// on which a negative count raises, each `UnityVersion(x)` its integer.
fn read_versions(
    s: &mut Stream<'_>,
    count: i32,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<UnityVersion>, TpkError> {
    let count = bounded("versions", struct_count(count, "the version list")?)?;
    let mut versions = MeteredVec::new(meter);
    for _ in 0..count {
        versions
            .push(read_version(s, "a version")?)
            .map_err(memory)?;
    }
    Ok(versions)
}

/// `read_version(stream)` (`Tpk.py:463-464`): `UnityVersion(UINT64...)`, the
/// stored integer kept as it is.
fn read_version(s: &mut Stream<'_>, what: &'static str) -> Result<UnityVersion, TpkError> {
    s.u64(what).map(UnityVersion::from_packed)
}

/// UnityPy's `BytesIO` over the payload, read forward. Each read takes the
/// bytes it asks for or fails the load: where `stream.read(n)` comes back
/// short, the `struct.unpack` or the index after it raises.
pub(crate) struct Stream<'a> {
    rest: &'a [u8],
}

impl<'a> Stream<'a> {
    /// `stream.read(n)`, whole.
    fn read(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], TpkError> {
        let (head, rest) = self
            .rest
            .split_at_checked(n)
            .ok_or_else(|| ends_in(what, n, self.rest.len()))?;
        self.rest = rest;
        Ok(head)
    }

    /// What is left of the payload.
    fn left(&self) -> usize {
        self.rest.len()
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], TpkError> {
        let (head, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or_else(|| ends_in(what, N, self.rest.len()))?;
        self.rest = rest;
        Ok(*head)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, TpkError> {
        self.array(what).map(|[b]| b)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, TpkError> {
        self.array(what).map(u16::from_le_bytes)
    }

    fn i16(&mut self, what: &'static str) -> Result<i16, TpkError> {
        self.array(what).map(i16::from_le_bytes)
    }

    fn i32(&mut self, what: &'static str) -> Result<i32, TpkError> {
        self.array(what).map(i32::from_le_bytes)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, TpkError> {
        self.array(what).map(u32::from_le_bytes)
    }

    fn i64(&mut self, what: &'static str) -> Result<i64, TpkError> {
        self.array(what).map(i64::from_le_bytes)
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, TpkError> {
        self.array(what).map(u64::from_le_bytes)
    }
}

/// A read past the payload's end: `struct.error`, or an `IndexError` for
/// `stream.read(1)[0]`.
fn ends_in(what: &'static str, n: usize, left: usize) -> TpkError {
    TpkError::Malformed(
        format!("the payload ends in {what}: {n} bytes to read, {left} left").into(),
    )
}

/// A count `Tpk.py` reads with `range(count)`: a negative one reads no
/// records.
fn range_count(n: i32) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// A count `Tpk.py` reads as a struct format (`f"<{count}Q"`), on which a
/// negative one raises `struct.error`.
fn struct_count(n: i32, what: &'static str) -> Result<u64, TpkError> {
    u64::try_from(n).map_err(|_| {
        TpkError::Malformed(format!("struct.error: a negative count ({n}) for {what}").into())
    })
}

/// `count` records of `table`, refused above [`TpkProvider::MAX_RECORDS`]
/// when the count is read, before any of its records.
fn bounded(table: &'static str, count: u64) -> Result<usize, TpkError> {
    let max = u64::try_from(TpkProvider::MAX_RECORDS).unwrap_or(u64::MAX);
    match usize::try_from(count) {
        Ok(n) if count <= max => Ok(n),
        _ => Err(TpkError::TooLarge {
            what: table,
            value: count,
            max,
        }),
    }
}

/// A growth the allocator refused. The provider's meter is unlimited
/// (crate docs, "Memory"), so a charge fails only when the allocation does.
pub(crate) fn memory(e: ChargeError) -> TpkError {
    TpkError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))
}
