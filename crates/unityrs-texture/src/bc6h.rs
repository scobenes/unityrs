//! BC6H signed and unsigned, implemented from Khronos Data Format Specification
//! 1.3 section 20.2 (Tables 114, 118, 119, 121, 122 and Equation 2):
//! <https://registry.khronos.org/DataFormat/specs/1.3/dataformat.1.3.html#BPTC>
//! The independent fixture oracle is bcdec; no third-party decoder is ported
//! here. Everything uses bounded stack arrays and checked slice access.
//!
//! **For speed**, each mode's decode is its own copy, its fields' places
//! constants; each block's 16 colours are interpolated once, and pixels
//! pick them by an index read at a fixed place.
mod tables;

use crate::HdrError;
use crate::place::put_block_data;

struct Mode {
    number: u32,
    transformed: bool,
    two: bool,
    bits: u32,
    delta: [u32; 3],
    // (endpoint scalar, low target bit, width); negative width reverses bits.
    fields: &'static [(usize, u32, i8)],
}

/// No mode: what [`mode`] gives for an index past [`tables::MODES`] (never).
const NONE: Mode = Mode {
    number: 0,
    transformed: false,
    two: false,
    bits: 16,
    delta: [16, 16, 16],
    fields: &[],
};

/// The `M`th of [`tables::MODES`] (never missing for the decoders' `M`).
#[allow(clippy::inline_always, reason = "measured: the mode's fields folded")]
#[inline(always)]
fn mode<const M: usize>() -> &'static Mode {
    tables::MODES.get(M).unwrap_or(&NONE)
}

/// Field `k` of mode `M`'s endpoints, with its place: `(endpoint scalar,
/// low target bit, width, first bit)`, the width negative where the
/// field's bits are reversed; `(0, 0, 0, 0)` past the mode's fields. A
/// constant for each, so each read is a constant shift.
const fn place<const M: usize>(k: usize) -> (usize, u32, i8, u32) {
    let mode = mode_at(M);
    let mut at = if mode.number < 2 { 2 } else { 5 };
    let (mut rest, mut i) = (mode.fields, 0);
    while let [(target, low, width), tail @ ..] = rest {
        if i == k {
            return (*target, *low, *width, at);
        }
        at += width.unsigned_abs() as u32;
        (rest, i) = (tail, i + 1);
    }
    (0, 0, 0, 0)
}

/// [`mode`] at compile time.
const fn mode_at(m: usize) -> &'static Mode {
    let (mut rest, mut i): (&'static [Mode], usize) = (tables::MODES, 0);
    while let [mode, tail @ ..] = rest {
        if i == m {
            return mode;
        }
        (rest, i) = (tail, i + 1);
    }
    &NONE
}

/// Mode `M`'s endpoint scalars as the block stores them: each field read
/// at its constant place ([`place`]), the fields unrolled (no mode has
/// more than 23).
#[allow(clippy::inline_always, reason = "measured: the mode's fields folded")]
#[inline(always)]
fn raw_endpoints<const M: usize>(word: u128) -> [i32; 12] {
    let mut raw = [0u32; 12];
    macro_rules! fields {
        ($($k:literal)*) => {$(
            let (target, low, width, at) = const { place::<M>($k) };
            if width != 0 {
                let count = u32::from(width.unsigned_abs());
                #[allow(clippy::cast_possible_truncation, reason = "masked to 10 bits")]
                let mut value = ((word >> at) as u32) & ((1 << count) - 1);
                if width < 0 {
                    value = value.reverse_bits() >> (32 - count);
                }
                if let Some(e) = raw.get_mut(target) {
                    *e |= value << low;
                }
            }
        )*};
    }
    fields!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22);
    // Each at most 16 bits.
    raw.map(|v| i32::try_from(v).unwrap_or(0))
}

fn sign_extend(value: i32, bits: u32) -> i32 {
    let sign = 1i32 << (bits - 1);
    (value ^ sign) - sign
}

fn unquantize(value: i32, bits: u32, signed: bool) -> i32 {
    if signed {
        if bits >= 16 {
            return value;
        }
        let magnitude = value.abs();
        let expanded = if magnitude == 0 {
            0
        } else if magnitude >= (1 << (bits - 1)) - 1 {
            0x7fff
        } else {
            ((magnitude << 15) + 0x4000) >> (bits - 1)
        };
        if value < 0 { -expanded } else { expanded }
    } else if bits >= 15 || value == 0 {
        value
    } else if value == (1 << bits) - 1 {
        0xffff
    } else {
        ((value << 15) + 0x4000) >> (bits - 1)
    }
}

/// The interpolated value as binary16 bits: at most 0x7BFF unsigned, and
/// signed at most 0x7BFF in magnitude, with the sign bit.
fn finish(value: i32, signed: bool) -> u16 {
    let bits = if signed {
        ((value.abs() * 31) >> 5) | if value < 0 { 0x8000 } else { 0 }
    } else {
        (value * 31) >> 6
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "at most 0xFBFF"
    )]
    let bits = bits as u16;
    bits
}

/// A block decoded but for its pixels: its 16 colours (the two-region
/// modes' 8 per region, region 1's after region 0's; the one-region
/// modes' 16), each channel's interpolated value as `finish` takes it to
/// the caller's form, and pixel `i`'s colour, its key, in bits `4 i` to
/// `4 i + 3` of `keys`.
struct Parsed<T> {
    colours: [T; 16],
    keys: u64,
}

/// The block of mode `M` (of [`tables::MODES`]), `S` its signedness: the
/// endpoints read field by field at constant places, transformed and
/// unquantized; each colour interpolated once and made the caller's by
/// `colour`; and the indices with each anchor's missing high bit put back
/// as 0, so every pixel's index is at a fixed place and no read waits on
/// the last.
#[allow(clippy::inline_always, reason = "measured: the mode's fields folded")]
#[inline(always)]
fn parse<const M: usize, const S: bool, T: Copy + Default>(
    word: u128,
    colour: impl Fn([i32; 3]) -> T,
) -> Parsed<T> {
    let mode = mode::<M>();
    let raw = raw_endpoints::<M>(word);
    let count = if mode.two { 12 } else { 6 };
    let mut ends = [0i32; 12];
    for (i, (end, &value)) in ends.iter_mut().zip(&raw).enumerate().take(count) {
        let c = i % 3;
        let mut value = value;
        if i >= 3 && mode.transformed {
            let base = raw.get(c).copied().unwrap_or(0);
            let delta = mode.delta.get(c).copied().unwrap_or(1);
            value = (base + sign_extend(value, delta)) & ((1 << mode.bits) - 1);
        }
        if S {
            value = sign_extend(value, mode.bits);
        }
        *end = unquantize(value, mode.bits, S);
    }
    let weights: &[i32] = if mode.two {
        &[0, 9, 18, 27, 37, 46, 55, 64]
    } else {
        &[0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64]
    };
    let mut colours = [T::default(); 16];
    for (region, colours) in colours
        .chunks_exact_mut(weights.len())
        .enumerate()
        .take(count / 6)
    {
        let [r0, g0, b0, r1, g1, b1] = ends
            .get(region * 6..region * 6 + 6)
            .and_then(|e| <[i32; 6]>::try_from(e).ok())
            .unwrap_or_default();
        for (out, &weight) in colours.iter_mut().zip(weights) {
            let lerp = |low: i32, high: i32| ((64 - weight) * low + weight * high + 32) >> 6;
            *out = colour([lerp(r0, r1), lerp(g0, g1), lerp(b0, b1)]);
        }
    }
    #[allow(clippy::cast_possible_truncation, reason = "the index bits")]
    let keys = if mode.two {
        // Bits 77 to 81 the partition, 46 index bits from 82: pixel 0's
        // and the anchor's 2 bits, the rest 3. Each pixel's key is its
        // region (bit 3) and index.
        let partition = ((word >> 77) & 31) as usize;
        let anchor = tables::ANCHORS.get(partition).copied().unwrap_or(15);
        let regions = u64::from(tables::PARTITIONS.get(partition).copied().unwrap_or(0));
        let x = (word >> 82) as u64;
        let x = (x & 3) | (x & !3) << 1;
        let low = (1u64 << (3 * anchor + 2)) - 1;
        let x = (x & low) | (x & !low) << 1;
        let mut keys = 0;
        for i in 0..16 {
            keys |= ((x >> (3 * i) & 7) | (regions >> i & 1) << 3) << (4 * i);
        }
        keys
    } else {
        // 63 index bits from 65: pixel 0's 3 bits, the rest 4.
        let x = (word >> 65) as u64;
        (x & 7) | (x & !7) << 1
    };
    Parsed { colours, keys }
}

/// [`parse`] for the block's mode; `None` for a reserved mode.
#[allow(clippy::inline_always, reason = "measured: one dispatch per block")]
#[inline(always)]
fn parse_any<const S: bool, T: Copy + Default>(
    word: u128,
    colour: impl Fn([i32; 3]) -> T,
) -> Option<Parsed<T>> {
    #[allow(clippy::cast_possible_truncation, reason = "masked to 5 bits")]
    let prefix = (word & 31) as u32;
    let number = if prefix & 3 < 2 { prefix & 3 } else { prefix };
    Some(match number {
        0 => parse::<0, S, T>(word, colour),
        1 => parse::<1, S, T>(word, colour),
        2 => parse::<2, S, T>(word, colour),
        3 => parse::<3, S, T>(word, colour),
        6 => parse::<4, S, T>(word, colour),
        7 => parse::<5, S, T>(word, colour),
        10 => parse::<6, S, T>(word, colour),
        11 => parse::<7, S, T>(word, colour),
        14 => parse::<8, S, T>(word, colour),
        15 => parse::<9, S, T>(word, colour),
        18 => parse::<10, S, T>(word, colour),
        22 => parse::<11, S, T>(word, colour),
        26 => parse::<12, S, T>(word, colour),
        30 => parse::<13, S, T>(word, colour),
        _ => return None,
    })
}

/// The colour of `key` (4 bits).
fn colour<T: Copy + Default>(colours: &[T; 16], key: u64) -> T {
    #[allow(clippy::cast_possible_truncation, reason = "masked to 4 bits")]
    colours
        .get((key & 15) as usize)
        .copied()
        .unwrap_or_default()
}

/// Raw binary16 RGB values in stored raster order. Only reserved modes fail.
pub(crate) fn block(data: &[u8; 16], signed: bool) -> Option<[[u16; 3]; 16]> {
    let word = u128::from_le_bytes(*data);
    let parsed = if signed {
        parse_any::<true, _>(word, |c| c.map(|v| finish(v, true)))?
    } else {
        parse_any::<false, _>(word, |c| c.map(|v| finish(v, false)))?
    };
    let mut result = [[0; 3]; 16];
    for (i, out) in result.iter_mut().enumerate() {
        *out = colour(&parsed.colours, parsed.keys >> (4 * i));
    }
    Some(result)
}

/// Unity's unsigned BC6H form, cropped to dimensions and vertically flipped.
/// Planning has already charged and allocated the exact RGB output.
pub(crate) fn decode(data: &[u8], w: usize, h: usize, out: &mut [u8]) -> Result<(), HdrError> {
    decode_image::<false>(data, w, h, out)
}

/// Explicit signed BC6H image path; Unity's `TextureFormat` does not distinguish it.
pub(crate) fn decode_signed(
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
) -> Result<(), HdrError> {
    decode_image::<true>(data, w, h, out)
}

/// An interpolated colour as the image's bytes, `[r, g, b, 0]` in a
/// `u32`: each channel [`finish`]ed and converted by
/// [`crate::tierb::half_byte`], a negative value (signed only) 0.
#[allow(clippy::inline_always, reason = "measured: every colour's conversion")]
#[inline(always)]
fn colour_bytes<const S: bool>(c: [i32; 3]) -> u32 {
    let [r, g, b] = c.map(|v| {
        if S && v < 0 {
            0
        } else {
            // The sign bit clear.
            crate::tierb::unsigned_half_byte(finish(v, S))
        }
    });
    u32::from_le_bytes([r, g, b, 0])
}

/// The image's blocks, in order, each converted to bytes
/// ([`crate::tierb::half_byte`]: a negative value 0) and written straight
/// to its flipped rows ([`put_block_data`]); the first reserved block refuses
/// the image.
fn decode_image<const S: bool>(
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
) -> Result<(), HdrError> {
    if w == 0 || h == 0 {
        return Ok(());
    }
    let invalid = HdrError::InvalidBlock;
    let length = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(3))
        .ok_or(invalid)?;
    let out = out.get_mut(..length).ok_or(invalid)?;
    let (across, down) = (w.div_ceil(4), h.div_ceil(4));
    let needed = across
        .checked_mul(down)
        .and_then(|n| n.checked_mul(16))
        .ok_or(invalid)?;
    let data = data.get(..needed).ok_or(invalid)?;
    let blocks = data.as_chunks::<16>().0;
    let mut reserved = false;
    put_block_data::<16, 12>(out, w * 3, h, (across, down), blocks, |b, rows| {
        let Some(parsed) = parse_any::<S, u32>(u128::from_le_bytes(*b), colour_bytes::<S>) else {
            reserved = true;
            return false;
        };
        let colours = parsed.colours;
        for (y, row) in rows.into_iter().enumerate() {
            let keys = parsed.keys >> (16 * y);
            let [p0, p1, p2, p3] = [0, 4, 8, 12].map(|at| u64::from(colour(&colours, keys >> at)));
            let v = u128::from(p0 | p1 << 24 | p2 << 48) | u128::from(p2 >> 16 | p3 << 8) << 64;
            if let Some(bytes) = v.to_le_bytes().first_chunk::<12>() {
                *row = *bytes;
            }
        }
        true
    });
    if reserved {
        return Err(HdrError::ReservedMode);
    }
    Ok(())
}
