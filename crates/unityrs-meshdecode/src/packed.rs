// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/PackedBitVector.py's unpack_ints and unpack_floats. See THIRD_PARTY_LICENSES.
//! A `PackedBitVector`'s items, as `unpack_ints` and `unpack_floats` give
//! them (`helpers/PackedBitVector.py:20-85`).
//!
//! **Integers** ([`unpack_ints`], `:20-65`). A little-endian bit stream:
//! item `i` is the `m_BitSize` bits from bit `m_BitSize * (start + i)` of
//! `m_Data`, least significant first, read a byte at a time, so an item
//! that needs a byte past `m_Data` raises `IndexError` (a list's message,
//! or `bytes`' for `TypelessData`). `count`, when not given, is
//! `m_NumItems`. A bit size of 0 reads nothing and gives zeros.
//! `m_BitSize` absent (`None`) fails the function's assert.
//!
//! **Floats** ([`unpack_floats`], `:68-85`). The assert that `m_BitSize`,
//! `m_Range` and `m_Start` are present, in that order; then at bit size 0
//! `m_Start`, `count` (or `m_NumItems`) times; otherwise each integer `x`
//! as `x * scale + m_Start` in `f64`, with `scale` the quotient
//! `m_Range / (2**m_BitSize - 1)`, computed once, the division first
//! (`PARITY.md` section 15.3: multiplying first rounds differently). No
//! step is fused: each is one IEEE 754 operation, as `CPython`'s.
//!
//! **NaNs.** A NaN operand gives its own NaN, quieted, the first operand's
//! where both are NaN, as x86-64's SSE and `AArch64` give it ([`nan`]);
//! the compiler may reorder an operation's operands, so this is done
//! by hand. A NaN no operand holds (`0 * inf`) is the host's own, as it is
//! for UnityPy.
//!
//! **What is refused.** An item wider than 32 bits ([`MeshError::Malformed`],
//! `PARITY.md` section 15.3): Unity packs at most 32 bits an item
//! (AssetStudio v0.16.47's MIT `PackedIntVector` and `PackedFloatVector`,
//! `Classes/AnimationClip.cs`, read them into 32-bit integers), and the
//! handler's integer outputs are 32-bit; and an `m_Data` value that is no
//! byte, or an `m_Range` or `m_Start` that is no float, which no typetree
//! holds.
//!
//! **Bounds.** The output is charged to the caller's meter and then counted
//! as decode work, a unit an item, before any item is read; a vector too
//! short for the items asked is answered (`IndexError`) before either, after
//! a scan of `m_Data`'s bytes, each counted.

use std::sync::Arc;

use unityrs_core::limits::{Meter, MeteredVec};

use crate::error::{IndexKind, MeshError};
use crate::vertex::field;
use crate::view::Obj;

/// The widest item unpacked (module docs).
const MAX_BITS: u64 = 32;

/// A NaN result as x86-64 and `AArch64` give it from `a` and `b`
/// (module docs): the first NaN operand, quieted; `r` itself otherwise.
fn nan(r: f64, a: f64, b: f64) -> f64 {
    const QUIET: u64 = 0x0008_0000_0000_0000;
    if !r.is_nan() {
        r
    } else if a.is_nan() {
        f64::from_bits(a.to_bits() | QUIET)
    } else if b.is_nan() {
        f64::from_bits(b.to_bits() | QUIET)
    } else {
        r
    }
}

/// `a + b`, a NaN as [`nan`] gives it.
pub(crate) fn add(a: f64, b: f64) -> f64 {
    nan(a + b, a, b)
}

/// `a - b`, a NaN as [`nan`] gives it.
pub(crate) fn sub(a: f64, b: f64) -> f64 {
    nan(a - b, a, b)
}

/// `a * b`, a NaN as [`nan`] gives it.
pub(crate) fn mul(a: f64, b: f64) -> f64 {
    nan(a * b, a, b)
}

/// `a / b`, a NaN as [`nan`] gives it.
pub(crate) fn div(a: f64, b: f64) -> f64 {
    nan(a / b, a, b)
}

/// `n` as a `usize`, saturated: a count past memory, which the meter
/// refuses.
pub(crate) fn size(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// `unpack_ints(packed, start, count)` (module docs): the items, charged
/// to `meter`.
pub(crate) fn unpack_ints(
    packed: &Obj<'_>,
    start: u64,
    count: Option<u64>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u32>, MeshError> {
    // UnityPy PackedBitVector.py:26-36
    let bits = packed
        .uint("m_BitSize", u64::from(u8::MAX))?
        .ok_or(MeshError::Assertion(""))?;
    let data = packed
        .ints("m_Data")?
        .ok_or(MeshError::NoAttribute("m_Data"))?;
    let count = match count {
        Some(n) => n,
        None => field(packed, "m_NumItems", u64::from(u32::MAX))?,
    };
    if bits > MAX_BITS && count > 0 {
        return Err(MeshError::malformed("a packed item wider than 32 bits"));
    }
    // UnityPy PackedBitVector.py:31-33: Python's ints, so a `u128` here.
    let first = u128::from(bits) * u128::from(start);
    let end = u128::from(bits) * (u128::from(start) + u128::from(count));
    let len = data.len();
    // The last byte the items read, past `m_Data`: Python reads up to it a
    // byte at a time, then raises. Every byte before it is read first.
    if bits > 0 && count > 0 && end.div_ceil(8) > len as u128 {
        let from = size(u64::try_from(first / 8).unwrap_or(u64::MAX)).min(len);
        meter.count_decode(u64::try_from(len - from).unwrap_or(u64::MAX))?;
        for i in from..len {
            data.item(i)?;
        }
        return Err(MeshError::Index(if data.is_bytes() {
            IndexKind::Bytes
        } else {
            IndexKind::List
        }));
    }
    // Charged, then counted: a refused charge counts no work.
    let mut out = MeteredVec::<u32>::with_capacity(meter, size(count))?;
    meter.count_decode(count)?;
    // UnityPy PackedBitVector.py:52-63
    let mut index = size(u64::try_from(first / 8).unwrap_or(u64::MAX));
    let mut bit = u32::try_from(first % 8).unwrap_or(0);
    let width = u32::try_from(bits).unwrap_or(0);
    for _ in 0..count {
        let mut got = 0u32;
        let mut value = 0u64;
        while got < width {
            let byte = data.item(index)?.ok_or(MeshError::Index(IndexKind::List))?;
            value |= u64::from(byte >> bit) << got;
            let num = (width - got).min(8 - bit);
            bit += num;
            got += num;
            if bit == 8 {
                index += 1;
                bit = 0;
            }
        }
        let mask = (1u64 << width) - 1;
        out.push(u32::try_from(value & mask).unwrap_or(u32::MAX))?;
    }
    Ok(out)
}

/// `unpack_floats(packed, start, count)` (module docs): the items as
/// `float`s, charged to `meter`.
pub(crate) fn unpack_floats(
    packed: &Obj<'_>,
    start: u64,
    count: Option<u64>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<f64>, MeshError> {
    // UnityPy PackedBitVector.py:74: `and` stops at the first `None`.
    let bits = packed
        .uint("m_BitSize", u64::from(u8::MAX))?
        .ok_or(MeshError::Assertion(""))?;
    let range = packed.float("m_Range")?.ok_or(MeshError::Assertion(""))?;
    let first = packed.float("m_Start")?.ok_or(MeshError::Assertion(""))?;
    if bits == 0 {
        // UnityPy PackedBitVector.py:77-78
        let n = match count {
            Some(n) => n,
            None => field(packed, "m_NumItems", u64::from(u32::MAX))?,
        };
        let mut out = MeteredVec::<f64>::with_capacity(meter, size(n))?;
        meter.count_decode(n)?;
        for _ in 0..n {
            out.push(first)?;
        }
        return Ok(out);
    }
    // UnityPy PackedBitVector.py:81-83
    let ints = unpack_ints(packed, start, count, meter)?;
    let mut out = MeteredVec::<f64>::with_capacity(meter, ints.len())?;
    meter.count_decode(u64::try_from(ints.len()).unwrap_or(u64::MAX))?;
    // `(1 << bits) - 1`, at most 2^32 - 1, exact in an `f64`.
    let steps = f64::from(u32::try_from((1u64 << bits.min(MAX_BITS)) - 1).unwrap_or(u32::MAX));
    let scale = div(range, steps);
    for &x in ints.as_slice() {
        out.push(add(mul(f64::from(x), scale), first))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{add, div, mul, sub};

    /// A NaN operand's own NaN, quieted, the first's where both are.
    #[test]
    fn a_nan_operand_gives_its_own_nan() {
        let a = f64::from_bits(0x7ff8_0000_0000_0001);
        let b = f64::from_bits(0xfff0_0000_0000_0002);
        assert_eq!(add(a, b).to_bits(), a.to_bits());
        assert_eq!(sub(b, a).to_bits(), 0xfff8_0000_0000_0002);
        assert_eq!(mul(2.0, b).to_bits(), 0xfff8_0000_0000_0002);
        assert_eq!(div(a, 3.0).to_bits(), a.to_bits());
        assert_eq!(add(1.5, 2.25), 3.75);
    }
}
