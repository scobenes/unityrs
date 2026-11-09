// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! The blob's strings: `TpkCommonString` (`helpers/Tpk.py:413-423`), the
//! `TpkStringBuffer` (`:407-410`) and `read_string` (`:445-456`).

use std::sync::Arc;

use unityrs_core::RawStr;
use unityrs_core::limits::{Meter, MeteredVec};

use super::{Stream, bounded, memory, range_count, read_version, struct_count};
use crate::TpkError;

/// Why a string UnityPy reads is refused: its length runs past the
/// payload's end (UnityPy decodes what is left, `stream.read(length)`).
pub(crate) const PAST_THE_END: &str = "a string longer than the rest of the payload";

/// Why a string UnityPy reads is refused: its 7-bit length takes more than
/// 5 bytes (UnityPy reads on, `:449-454`).
pub(crate) const LONG_PREFIX: &str = "a string length of more than 5 bytes";

/// `TpkCommonString(stream)` (`Tpk.py:418-423`): the version information
/// (a `range` count of versions, each with a count byte), read and not kept,
/// since nothing in UnityPy 1.25.2 calls `GetCount` and `get_common_strings()`
/// takes every index (`GetStrings`, `:425-426`); then the indices, a struct
/// format (`f"<{indicesCount}H"`: a negative count raises).
pub(crate) fn read_common_string(
    s: &mut Stream<'_>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u16>, TpkError> {
    let count = bounded(
        "common-string versions",
        range_count(s.i32("the common-string version count")?),
    )?;
    for _ in 0..count {
        read_version(s, "a common-string version")?;
        s.u8("a common-string count")?;
    }
    let count = s.i32("the common-string index count")?;
    let count = bounded(
        "common-string indices",
        struct_count(count, "the common-string indices")?,
    )?;
    let (ids, _) = s
        .read(count * 2, "the common-string indices")?
        .as_chunks::<2>();
    let mut indices = MeteredVec::new(meter);
    indices.reserve(ids.len()).map_err(memory)?;
    for id in ids {
        indices.push(u16::from_le_bytes(*id)).map_err(memory)?;
    }
    Ok(indices)
}

/// `TpkStringBuffer(stream)` (`Tpk.py:408-410`): a count (a `range`: a
/// negative one is none), then each string.
pub(crate) fn read_string_buffer(
    s: &mut Stream<'_>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<RawStr>, TpkError> {
    let count = bounded("strings", range_count(s.i32("the string count")?))?;
    let mut strings = MeteredVec::new(meter);
    for _ in 0..count {
        let string = read_string(s, meter)?;
        strings.push(string).map_err(memory)?;
    }
    Ok(strings)
}

/// `read_string(stream)` (`Tpk.py:445-456`): a little-endian 7-bit length,
/// then that many bytes, decoded as UTF-8 (`UnicodeDecodeError` otherwise).
/// Where UnityPy reads on, this refuses (`PARITY.md` section 19.3): a length
/// of more than 5 bytes ([`LONG_PREFIX`]), and a length past the payload's
/// end ([`PAST_THE_END`]). The string is copied, charged first, only once
/// its bytes are known to be there, so a length can never size an
/// allocation.
fn read_string(s: &mut Stream<'_>, meter: &Arc<Meter>) -> Result<RawStr, TpkError> {
    let mut length = 0_u64;
    let mut shift = 0;
    loop {
        // `(i,) = stream.read(1)`.
        let i = s.u8("a string's length")?;
        length |= u64::from(i & 0x7F) << shift;
        if i & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 35 {
            return Err(TpkError::Malformed(LONG_PREFIX.into()));
        }
    }
    let length = usize::try_from(length)
        .ok()
        .filter(|&n| n <= s.left())
        .ok_or_else(|| TpkError::Malformed(PAST_THE_END.into()))?;
    let bytes = s.read(length, "a string")?;
    // `.decode("utf-8")`.
    std::str::from_utf8(bytes)
        .map_err(|e| TpkError::Malformed(format!("UnicodeDecodeError: {e}").into()))?;
    RawStr::charged(bytes, meter).map_err(memory)
}
