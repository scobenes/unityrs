// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/BcnDecode.c, its BC7 block decoder (decode_bc7_block, bc7_mode_info and its tables); that file is dedicated to the public domain (CC0) in its own header, and Pillow's licence covers the rest. See THIRD_PARTY_LICENSES.
//! Pillow 9.5.0's BC7 block decoder (`decode_bc7_block`), for
//! [`super::decode`]: the rules are in [`super`]'s documentation. A block's
//! colours are computed once per subset and index into a palette, and the
//! pixels are its entries.

use super::pick;

/// A block's 16 pixels, each `u32::from_le_bytes([r, g, b, a])`.
type Pixels = [u32; 16];

/// `bc7_mode_info`: subsets, partition bits, rotation bits, index-selection
/// bits, colour bits, alpha bits, p-bits per endpoint, p-bits per subset,
/// index bits and second index bits.
#[derive(Clone, Copy)]
struct ModeInfo {
    ns: u32,
    pb: u32,
    rb: u32,
    isb: u32,
    cb: u32,
    ab: u32,
    epb: u32,
    spb: u32,
    ib: u32,
    ib2: u32,
}

const fn mode_info(v: [u32; 10]) -> ModeInfo {
    let [ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2] = v;
    ModeInfo {
        ns,
        pb,
        rb,
        isb,
        cb,
        ab,
        epb,
        spb,
        ib,
        ib2,
    }
}

/// `bc7_modes`.
const BC7_MODES: [ModeInfo; 8] = [
    mode_info([3, 4, 0, 0, 4, 0, 1, 0, 3, 0]),
    mode_info([2, 6, 0, 0, 6, 0, 0, 1, 3, 0]),
    mode_info([3, 6, 0, 0, 5, 0, 0, 0, 2, 0]),
    mode_info([2, 6, 0, 0, 7, 0, 1, 0, 2, 0]),
    mode_info([1, 0, 2, 1, 5, 6, 0, 0, 2, 3]),
    mode_info([1, 0, 2, 0, 7, 8, 0, 0, 2, 2]),
    mode_info([1, 0, 0, 0, 7, 7, 1, 0, 4, 0]),
    mode_info([2, 6, 0, 0, 5, 5, 1, 0, 2, 0]),
];

/// `bc7_si2`: the subset of each pixel, one bit each, by partition.
#[rustfmt::skip]
const BC7_SI2: [u32; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec,
    0xfe80, 0xe800, 0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce,
    0x008c, 0x7310, 0x3100, 0x8cce, 0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0,
    0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc, 0x3c3c, 0x55aa, 0x9696, 0xa55a,
    0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660, 0x0272, 0x04e4,
    0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336, 0x9cc6, 0x817e, 0xe718,
    0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// `bc7_si3`: the subset of each pixel, two bits each, by partition.
// Pillow's table as it writes it, so the two compare line for line.
#[allow(clippy::unreadable_literal)]
#[rustfmt::skip]
const BC7_SI3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0,
    0x5a5a5050, 0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494, 0xa4a4a4a4,
    0xa9a59450, 0x2a0a4250, 0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454,
    0x6a6a4040, 0xa4a45000, 0x1a1a0500, 0x0050a4a4, 0xaaa59090, 0x14696914, 0x69691400,
    0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200, 0xa9a58000, 0x5090a0a8, 0xa8a09050,
    0x24242424, 0x00aa5500, 0x24924924, 0x24499224, 0x50a50a50, 0x500aa550, 0xaaaa4444,
    0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600, 0xaa444444,
    0x54a854a8, 0x95809580, 0x96969600, 0xa85454a8, 0x80959580, 0xaa141414, 0x96960000,
    0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44,
    0x2a4a5254,
];

/// `bc7_ai0`: the second subset's anchor, two subsets.
#[rustfmt::skip]
const BC7_AI0: [u32; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2,  8, 2, 2, 8,
    8,  15, 2,  8,  2,  2,  8,  8,  2,  2,  15, 15, 6,  8,  2,  8,  15, 15, 2, 8, 2, 2,
    2,  15, 15, 6,  6,  2,  6,  8,  15, 15, 2,  2,  15, 15, 15, 15, 15, 2,  2, 15,
];

/// `bc7_ai1`: the second subset's anchor, three subsets.
#[rustfmt::skip]
const BC7_AI1: [u32; 64] = [
    3,  3,  15, 15, 8, 3,  15, 15, 8,  8,  6, 6,  6, 5,  3, 3,  3,  3,  8,  15, 3,  3,
    6,  10, 5,  8,  8, 6,  8,  5,  15, 15, 8, 15, 3, 5,  6, 10, 8,  15, 15, 3,  15, 5,
    15, 15, 15, 15, 3, 15, 5,  5,  5,  8,  5, 10, 5, 10, 8, 13, 15, 12, 3,  3,
];

/// `bc7_ai2`: the third subset's anchor.
#[rustfmt::skip]
const BC7_AI2: [u32; 64] = [
    15, 8,  8,  3,  15, 15, 3,  8,  15, 15, 15, 15, 15,
    15, 15, 8,  15, 8,  15, 3,  15, 8,  15, 8,  3,  15,
    6,  10, 15, 15, 10, 8,  15, 3,  15, 10, 10, 8,  9,
    10, 6,  15, 8,  15, 3,  6,  6,  8,  15, 3,  15, 15,
    15, 15, 15, 15, 15, 15, 15, 15, 3,  15, 15, 8,
];

/// `bc7_weights2`, `bc7_weights3`, `bc7_weights4`.
const BC7_WEIGHTS2: [u32; 4] = [0, 21, 43, 64];
const BC7_WEIGHTS3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const BC7_WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// `bc7_get_weights`.
fn bc7_weights(n: u32) -> &'static [u32] {
    match n {
        2 => &BC7_WEIGHTS2,
        3 => &BC7_WEIGHTS3,
        _ => &BC7_WEIGHTS4,
    }
}

/// `expand_quantized`: `v` widened from `bits` bits to 8 by replication, in
/// a `UINT8` as Pillow keeps it.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn expand_quantized(v: u32, bits: u32) -> u32 {
    let v = (v << 8u32.saturating_sub(bits)) & 0xff;
    v | v.checked_shr(bits).unwrap_or(0)
}

/// `get_bits`: `count` bits (at most 8) of the block from bit `bit`. Every
/// mode's fields fill the block's 128 bits exactly, so no read runs past
/// it.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn get_bits(block: u128, bit: u32, count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    let v = block.checked_shr(bit).unwrap_or(0) & ((1u128 << count) - 1);
    u32::try_from(v).unwrap_or(0)
}

/// A pair of endpoints as [`lerp_pixel`] takes them: each channel (under
/// 256) in a 16-bit lane of a `u64`, `64 e0 + 32` and `e1 - e0`.
type Lanes = (u64, u64);

/// [`Lanes`] of the endpoints `e0` and `e1`: the difference taken modulo
/// 2^64, so a lane's negative difference borrows from the next lane, but
/// the whole is still `sum (e1 - e0) 2^(16 c)`.
fn lerp_lanes(e0: [u32; 4], e1: [u32; 4]) -> Lanes {
    let lanes = |e: [u32; 4]| {
        e.iter()
            .rev()
            .fold(0u64, |v, &c| v << 16 | u64::from(c & 0xff))
    };
    let (l0, l1) = (lanes(e0), lanes(e1));
    (l0 * 64 + 0x0020_0020_0020_0020, l1.wrapping_sub(l0))
}

/// `bc7_lerp` at weight `s` of 64, `((64 - s) e0 + s e1 + 32) >> 6`, for
/// the channels `keep` holds the bytes of, the rest 0: the four channels
/// at once, as `64 e0 + 32 + s (e1 - e0)` in 16-bit lanes. Modulo 2^64 the
/// sum is `sum v 2^(16 c)`, and each lane's `v` is in 0 to 2^14, so it is
/// exactly the lanes; each `v >> 6` is its lane's low byte after the shift.
fn lerp_pixel((base, delta): Lanes, s: u32, keep: u32) -> u32 {
    let v = (base.wrapping_add(delta.wrapping_mul(u64::from(s))) >> 6) & 0x00FF_00FF_00FF_00FF;
    let v = (v | v >> 8) & 0x0000_FFFF_0000_FFFF;
    #[allow(clippy::cast_possible_truncation, reason = "the four bytes")]
    let v = (v | v >> 16) as u32;
    v & keep
}

/// The six endpoints a BC7 block holds at most (three subsets), each
/// `[r, g, b, a]`.
type Endpoints = [[u32; 4]; 6];

/// `decode_bc7_block`'s endpoints: read from bit `*bit` (moved past them),
/// channel by channel, then the p-bits, then each widened to 8 bits. Each
/// field's place is a constant of the mode, so every read is a constant
/// shift and none waits on the last.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_endpoints(block: u128, info: &ModeInfo, bit: &mut u32) -> Endpoints {
    let start = *bit;
    let numep = info.ns * 2;
    let (cb, ab) = (info.cb, info.ab);
    // The p-bits follow the colour and alpha fields: one an endpoint, or
    // one a subset.
    let pbits = start + numep * (3 * cb + ab);
    let (pcount, pwidth) = if info.epb != 0 {
        (numep, 1)
    } else if info.spb != 0 {
        (info.ns, 1)
    } else {
        (0, 0)
    };
    *bit = pbits + pcount;
    let mut endpoints: Endpoints = [[0; 4]; 6];
    for (e, endpoint) in (0u32..).zip(endpoints.iter_mut()) {
        if e >= numep {
            break;
        }
        // `ASSIGN_P`: the endpoint's p-bit appended to each channel, the
        // alpha's only where the mode has alpha bits.
        let p = if info.epb != 0 {
            get_bits(block, pbits + e, pwidth)
        } else {
            get_bits(block, pbits + e / 2, pwidth)
        };
        for (c, channel) in (0u32..).zip(endpoint.iter_mut()) {
            *channel = if c < 3 {
                let v = get_bits(block, start + (c * numep + e) * cb, cb);
                expand_quantized(((v << pwidth) | p) & 0xff, cb + pwidth)
            } else if ab != 0 {
                let v = get_bits(block, start + 3 * numep * cb + e * ab, ab);
                expand_quantized(((v << pwidth) | p) & 0xff, ab + pwidth)
            } else {
                255
            };
        }
    }
    endpoints
}

/// The pixels of `bc7_block`'s modes 4 and 5 (one subset, two index sets,
/// rotation and index selection): their two palettes, one per index set,
/// each holding the channels its set weights, rotated, so a pixel is the
/// two entries' union.
struct Split {
    first: [u32; 16],
    second: [u32; 16],
}

/// `decode_bc7_block`, into the block's rows of `[r, g, b, a]`.
pub(super) fn bc7_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let px = bc7_pixels(src);
    for (row, px) in rows.into_iter().zip(px.as_chunks::<4>().0) {
        for (dst, p) in row.as_chunks_mut::<4>().0.iter_mut().zip(px) {
            *dst = p.to_le_bytes();
        }
    }
}

/// `decode_bc7_block`'s pixels.
#[inline(never)]
fn bc7_pixels(src: &[u8; 16]) -> Pixels {
    let [first, ..] = *src;
    let block = u128::from_le_bytes(*src);
    // The mode is the number of unset bits before the first set bit; each
    // mode's decode is its own copy, its fields' widths constants.
    match first.trailing_zeros() {
        0 => bc7_mode::<0>(block),
        1 => bc7_mode::<1>(block),
        2 => bc7_mode::<2>(block),
        3 => bc7_mode::<3>(block),
        4 => bc7_mode::<4>(block),
        5 => bc7_mode::<5>(block),
        6 => bc7_mode::<6>(block),
        7 => bc7_mode::<7>(block),
        // The degenerate case: no mode bit.
        _ => [u32::from_le_bytes([0, 0, 0, 255]); 16],
    }
}

/// [`bc7_pixels`] for mode `M`.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_mode<const M: u32>(block: u128) -> Pixels {
    let Some(info) = BC7_MODES.get(M as usize) else {
        return [0; 16];
    };
    let mode = M;
    let mut bit = mode + 1;
    let partition = get_bits(block, bit, info.pb) as usize;
    bit += info.pb;
    let rotation = get_bits(block, bit, info.rb);
    bit += info.rb;
    let index_sel = get_bits(block, bit, info.isb);
    bit += info.isb;
    let endpoints = bc7_endpoints(block, info, &mut bit);
    // The index sets, from bit `bit` and 16 indices of `ib` bits on, each
    // anchor index one bit short; at most 63 bits each.
    let indices = |at: u32| -> u64 {
        #[allow(clippy::cast_possible_truncation, reason = "at most 63 bits")]
        let v = block.checked_shr(at).unwrap_or(0) as u64;
        v
    };
    let ci = indices(bit);
    if info.ab != 0 && info.ib2 != 0 {
        let mut ci = ci;
        let mut ai = indices(bit + 16 * info.ib - info.ns);
        let split = bc7_split(info, &endpoints, rotation, index_sel);
        let (m0, m1) = ((1u64 << info.ib) - 1, (1u64 << info.ib2) - 1);
        let mut px = [0; 16];
        for (i, p) in px.iter_mut().enumerate() {
            // Pixel 0 alone is an anchor: one subset, so each shift is a
            // constant once the loop is unrolled.
            let anchor = u32::from(i == 0);
            #[allow(clippy::cast_possible_truncation, reason = "masked to 4 bits")]
            let (i0, i1) = ((ci & m0 >> anchor) as usize, (ai & m1 >> anchor) as usize);
            ci >>= info.ib - anchor;
            ai >>= info.ib2 - anchor;
            *p = pick(&split.first, i0) | pick(&split.second, i1);
        }
        return px;
    }
    // Each subset's palette, 16 entries apart.
    let cw = bc7_weights(info.ib);
    let mut palette = [0u32; 64];
    for (s, entries) in palette
        .as_chunks_mut::<16>()
        .0
        .iter_mut()
        .take(info.ns as usize)
        .enumerate()
    {
        let lanes = lerp_lanes(pick(&endpoints, 2 * s), pick(&endpoints, 2 * s + 1));
        for (entry, &w) in entries.iter_mut().zip(cw) {
            *entry = lerp_pixel(lanes, w, u32::MAX);
        }
    }
    let (subsets, [a1, a2], sb) = bc7_layout(info.ns, partition);
    // The index set with its anchors' missing high bits put back as 0, so
    // pixel `i`'s index is at bit `i ib`, read at a fixed place, no read
    // waiting on the last. The anchors in order: pixel 0, then the others' (the further
    // subsets', as many as there are, in either order), each a high bit.
    let ci = unanchor(ci, info.ib, 0);
    let ci = match info.ns {
        1 => ci,
        2 => unanchor(ci, info.ib, a1),
        _ => unanchor(unanchor(ci, info.ib, a1.min(a2)), info.ib, a1.max(a2)),
    };
    let mask = (1u64 << info.ib) - 1;
    let mut px = [0; 16];
    for (i, p) in (0u32..).zip(px.iter_mut()) {
        #[allow(clippy::cast_possible_truncation, reason = "masked to 4 bits")]
        let k = (ci >> (i * info.ib) & mask) as usize;
        let s = ((subsets >> (i * sb)) & ((1 << sb) - 1)) as usize;
        *p = pick(&palette, (s << 4 | k) & 63);
    }
    px
}

/// `bc7_block`'s modes 4 and 5: the palettes of their two index sets.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_split(info: &ModeInfo, endpoints: &Endpoints, rotation: u32, index_sel: u32) -> Split {
    let [e0, e1, ..] = *endpoints;
    let lanes = lerp_lanes(e0, e1);
    let (cw, aw) = (bc7_weights(info.ib), bc7_weights(info.ib2));
    // The first index set weights the colour, the second the alpha, or
    // with the index selection the reverse.
    let (first_mask, second_mask) = if index_sel != 0 {
        (0xFF00_0000, 0x00FF_FFFF)
    } else {
        (0x00FF_FFFF, 0xFF00_0000)
    };
    let rotate = |v: u32| -> u32 {
        let mut b = v.to_le_bytes();
        match rotation {
            1 => b.swap(0, 3),
            2 => b.swap(1, 3),
            3 => b.swap(2, 3),
            _ => {}
        }
        u32::from_le_bytes(b)
    };
    let mut split = Split {
        first: [0; 16],
        second: [0; 16],
    };
    for (entry, &w) in split.first.iter_mut().zip(cw) {
        *entry = rotate(lerp_pixel(lanes, w, first_mask));
    }
    for (entry, &w) in split.second.iter_mut().zip(aw) {
        *entry = rotate(lerp_pixel(lanes, w, second_mask));
    }
    split
}

/// `bc7_get_subset` for every pixel of a partition, pixel `i`'s subset in
/// bits `i b` to `i b + b - 1`; the anchors of the second and third
/// subsets (the first's is pixel 0; 0 where there is no such subset); and
/// `b`, the bits a subset takes (0, 1 or 2).
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_layout(ns: u32, partition: usize) -> (u32, [u32; 2], u32) {
    match ns {
        2 => (pick(&BC7_SI2, partition), [pick(&BC7_AI0, partition), 0], 1),
        3 => (
            pick(&BC7_SI3, partition),
            [pick(&BC7_AI1, partition), pick(&BC7_AI2, partition)],
            2,
        ),
        _ => (0, [0, 0], 0),
    }
}

/// An index set read from the block, pixel `anchor`'s index one bit
/// short, with that pixel's high bit put back as 0: every bit from there
/// up moves one up. Applied to the anchors in increasing order, pixel
/// `i`'s `ib`-bit index is then at bit `i ib`.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn unanchor(indices: u64, ib: u32, anchor: u32) -> u64 {
    let at = anchor * ib + ib - 1;
    let low = (1u64 << at) - 1;
    (indices & low) | (indices & !low) << 1
}
