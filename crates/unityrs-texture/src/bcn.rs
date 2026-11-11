// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/BcnDecode.c, its BC1, BC3 and BC7 block decoders and its block order (decode_bc1_color, decode_bc3_alpha, decode_bc7_block, put_block, decode_bcn); that file is dedicated to the public domain (CC0) in its own header, and Pillow's licence covers the rest. See THIRD_PARTY_LICENSES.
//! Pillow 9.5.0's `BCn` decoders for DXT1 (`bcn` 1), DXT5 (`bcn` 3) and BC7
//! (`bcn` 7), which UnityPy 1.25.2 calls through `Image.frombytes` in mode
//! `RGBA` (`docs/plans/extract.md` E15), ported rule for rule:
//! - **BC1** (`decode_bc1_color`): the two 5:6:5 endpoints widened by bit
//!   replication; four colours when `c0 > c1` (the thirds, each `(2a + b) / 3`
//!   truncated), else three and transparent black (the half, truncated, then
//!   (0, 0, 0, 0)). Alpha 255 elsewhere.
//! - **BC3**: the colour block as BC1's, always in its four-colour form
//!   (`separate_alpha`), and the alpha block (`decode_bc3_alpha`): eight
//!   values when `a0 > a1` (sevenths, truncated), else six with 0 and 255.
//! - **BC7** (`decode_bc7_block`): Pillow's own reading of the modes, which
//!   departs from the specification in one place: a block whose first byte
//!   is 0 (no mode bit) gives (0, 0, 0, 255), where the specification gives
//!   (0, 0, 0, 0). The rest is the specification: per mode, the partition,
//!   rotation and index-selection bits, the endpoints channel by channel,
//!   the p-bits per endpoint or per subset, the endpoints widened by bit
//!   replication, the anchor indices one bit short, and the interpolation
//!   `((64 - w) * e0 + w * e1 + 32) >> 6`.
//!
//! **Block order** (`put_block`, `decode_bcn`): blocks fill the padded
//! image left to right, then top to bottom, each a 4x4 square; UnityPy then
//! crops it and flips it top to bottom. [`decode()`] writes each block's
//! pixels that fall inside the texture straight to their flipped rows.

/// One pixel, `[r, g, b, a]`.
type Rgba = [u8; 4];

/// Which of Pillow's decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `bcn` 1: DXT1.
    Bc1,
    /// `bcn` 3: DXT5.
    Bc3,
    /// `bcn` 7: BC7.
    Bc7,
}

/// The `i`th entry of `table`, or the default where it has none (never, for
/// the indices the decoders compute).
fn pick<T: Copy + Default>(table: &[T], i: usize) -> T {
    table.get(i).copied().unwrap_or_default()
}

/// A value Pillow keeps in a `UINT8` that never exceeds 255 here.
fn byte(v: u32) -> u8 {
    u8::try_from(v & 0xFF).unwrap_or(u8::MAX)
}

/// Decodes `data` (at least the padded image's blocks) into `out`, the
/// `w` x `h` image's rows top to bottom: block row `by` of the padded image
/// holds source rows `4 by` to `4 by + 3`, and source row `y` is output row
/// `h - 1 - y`.
pub(crate) fn decode(kind: Kind, data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let block_bytes = if kind == Kind::Bc1 { 8 } else { 16 };
    let (blocks_x, blocks_y) = (w.div_ceil(4), h.div_ceil(4));
    let row = w.saturating_mul(4);
    let mut blocks = data.chunks_exact(block_bytes);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let Some(src) = blocks.next() else {
                return;
            };
            let mut col = [[0u8; 4]; 16];
            match kind {
                Kind::Bc1 => bc1_color(src, &mut col, false),
                Kind::Bc3 => {
                    bc1_color(src.get(8..).unwrap_or_default(), &mut col, true);
                    bc3_alpha(src, &mut col);
                }
                Kind::Bc7 => bc7_block(src, &mut col),
            }
            for (n, px) in col.iter().enumerate() {
                let (x, y) = (bx * 4 + n % 4, by * 4 + n / 4);
                if x >= w || y >= h {
                    continue;
                }
                let at = (h - 1 - y).saturating_mul(row).saturating_add(x * 4);
                if let Some(dst) = out.get_mut(at..at.saturating_add(4)) {
                    dst.copy_from_slice(px);
                }
            }
        }
    }
}

/// `LOAD16`/`LOAD32`: a little-endian value of `src`'s bytes from `at`.
fn load(src: &[u8], at: usize, len: usize) -> u32 {
    (0..len).rev().fold(0, |v, i| {
        (v << 8) | u32::from(src.get(at + i).copied().unwrap_or(0))
    })
}

/// `decode_565`.
fn decode_565(x: u32) -> Rgba {
    let mut r = (x & 0xf800) >> 8;
    r |= r >> 5;
    let mut g = (x & 0x7e0) >> 3;
    g |= g >> 6;
    let mut b = (x & 0x1f) << 3;
    b |= b >> 5;
    [byte(r), byte(g), byte(b), 0xff]
}

/// `decode_bc1_color`: `src` is the 8-byte colour block.
fn bc1_color(src: &[u8], dst: &mut [Rgba; 16], separate_alpha: bool) {
    let (c0, c1, lut) = (load(src, 0, 2), load(src, 2, 2), load(src, 4, 4));
    let p0 = decode_565(c0);
    let p1 = decode_565(c1);
    let [r0, g0, b0, _] = p0.map(u32::from);
    let [r1, g1, b1, _] = p1.map(u32::from);
    let (p2, p3) = if c0 > c1 || separate_alpha {
        (
            [
                byte((2 * r0 + r1) / 3),
                byte((2 * g0 + g1) / 3),
                byte((2 * b0 + b1) / 3),
                0xff,
            ],
            [
                byte((r0 + 2 * r1) / 3),
                byte((g0 + 2 * g1) / 3),
                byte((b0 + 2 * b1) / 3),
                0xff,
            ],
        )
    } else {
        (
            [
                byte(u32::midpoint(r0, r1)),
                byte(u32::midpoint(g0, g1)),
                byte(u32::midpoint(b0, b1)),
                0xff,
            ],
            [0, 0, 0, 0],
        )
    };
    let p = [p0, p1, p2, p3];
    for (n, px) in dst.iter_mut().enumerate() {
        *px = pick(&p, ((lut >> (2 * n)) & 3) as usize);
    }
}

/// `decode_bc3_alpha`, unsigned, into each pixel's alpha: `src` is the BC3
/// block, whose first 8 bytes are the alpha block.
fn bc3_alpha(src: &[u8], dst: &mut [Rgba; 16]) {
    let (a0, a1) = (load(src, 0, 1), load(src, 1, 1));
    let (lut1, lut2) = (load(src, 2, 3), load(src, 5, 3));
    let a: [u8; 8] = if a0 > a1 {
        [
            byte(a0),
            byte(a1),
            byte((6 * a0 + a1) / 7),
            byte((5 * a0 + 2 * a1) / 7),
            byte((4 * a0 + 3 * a1) / 7),
            byte((3 * a0 + 4 * a1) / 7),
            byte((2 * a0 + 5 * a1) / 7),
            byte((a0 + 6 * a1) / 7),
        ]
    } else {
        [
            byte(a0),
            byte(a1),
            byte((4 * a0 + a1) / 5),
            byte((3 * a0 + 2 * a1) / 5),
            byte((2 * a0 + 3 * a1) / 5),
            byte((a0 + 4 * a1) / 5),
            0,
            0xff,
        ]
    };
    for (n, px) in dst.iter_mut().enumerate() {
        let lut = if n < 8 { lut1 } else { lut2 };
        px[3] = pick(&a, ((lut >> (3 * (n % 8))) & 7) as usize);
    }
}

/// `bc7_mode_info`: subsets, partition bits, rotation bits, index-selection
/// bits, colour bits, alpha bits, p-bits per endpoint, p-bits per subset,
/// index bits and second index bits.
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

/// `bc7_get_subset`.
fn bc7_subset(ns: u32, partition: usize, n: usize) -> usize {
    let v = match ns {
        2 => 1 & (pick(&BC7_SI2, partition) >> n),
        3 => 3 & (pick(&BC7_SI3, partition) >> (2 * n)),
        _ => 0,
    };
    v as usize
}

/// `expand_quantized`: `v` widened from `bits` bits to 8 by replication, in
/// a `UINT8` as Pillow keeps it.
fn expand_quantized(v: u32, bits: u32) -> u32 {
    let v = (v << 8u32.saturating_sub(bits)) & 0xff;
    v | v.checked_shr(bits).unwrap_or(0)
}

/// `get_bits`: `count` bits (at most 8) of the block from bit `bit`. Every
/// mode's fields fill the block's 128 bits exactly, so no read runs past
/// it.
fn get_bits(block: u128, bit: u32, count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    let v = block.checked_shr(bit).unwrap_or(0) & ((1u128 << count) - 1);
    u32::try_from(v).unwrap_or(0)
}

/// `bc7_lerp`.
fn bc7_lerp(e0: [u32; 4], e1: [u32; 4], s0: u32, s1: u32) -> Rgba {
    let (t0, t1) = (64 - s0, 64 - s1);
    let c = |i: usize, t: u32, s: u32| byte((t * pick(&e0, i) + s * pick(&e1, i) + 32) >> 6);
    [c(0, t0, s0), c(1, t0, s0), c(2, t0, s0), c(3, t1, s1)]
}

/// The six endpoints a BC7 block holds at most (three subsets), each
/// `[r, g, b, a]`.
type Endpoints = [[u32; 4]; 6];

/// `decode_bc7_block`'s endpoints: read from bit `*bit` (moved past them),
/// channel by channel, then the p-bits, then each widened to 8 bits.
fn bc7_endpoints(block: u128, info: &ModeInfo, bit: &mut u32) -> Endpoints {
    let mut load = |n: u32| {
        let v = get_bits(block, *bit, n);
        *bit += n;
        v
    };
    let (mut cb, mut ab) = (info.cb, info.ab);
    let numep = (info.ns * 2) as usize;
    let mut endpoints: Endpoints = [[0; 4]; 6];
    for channel in 0..3 {
        for e in endpoints.iter_mut().take(numep) {
            let v = load(cb);
            if let Some(c) = e.get_mut(channel) {
                *c = v;
            }
        }
    }
    for e in endpoints.iter_mut().take(numep) {
        e[3] = if ab != 0 { load(ab) } else { 255 };
    }
    // p-bits, each appended to its endpoints' channels (`ASSIGN_P`), the
    // alpha's only where the mode has alpha bits.
    let alpha = ab != 0;
    let assign_p = |e: &mut [u32; 4], val: u32| {
        for (i, c) in e.iter_mut().enumerate() {
            if i < 3 || alpha {
                *c = ((*c << 1) | val) & 0xff;
            }
        }
    };
    if info.epb != 0 {
        // Per endpoint.
        cb += 1;
        if ab != 0 {
            ab += 1;
        }
        for e in endpoints.iter_mut().take(numep) {
            let val = load(1);
            assign_p(e, val);
        }
    }
    if info.spb != 0 {
        // Per subset.
        cb += 1;
        if ab != 0 {
            ab += 1;
        }
        for pair in endpoints.as_chunks_mut::<2>().0.iter_mut().take(numep / 2) {
            let val = load(1);
            for e in pair {
                assign_p(e, val);
            }
        }
    }
    for e in endpoints.iter_mut().take(numep) {
        for (i, c) in e.iter_mut().enumerate() {
            if i < 3 {
                *c = expand_quantized(*c, cb);
            } else if ab != 0 {
                *c = expand_quantized(*c, ab);
            }
        }
    }
    endpoints
}

/// `decode_bc7_block`.
fn bc7_block(src: &[u8], col: &mut [Rgba; 16]) {
    let first = u32::from(src.first().copied().unwrap_or(0));
    if first == 0 {
        // The degenerate case: no mode bit.
        *col = [[0, 0, 0, 255]; 16];
        return;
    }
    let mut bytes = [0u8; 16];
    for (b, s) in bytes.iter_mut().zip(src) {
        *b = *s;
    }
    let block = u128::from_le_bytes(bytes);
    // The mode is the number of unset bits before the first set bit.
    let mode = first.trailing_zeros();
    let Some(info) = BC7_MODES.get(mode as usize) else {
        return;
    };
    let mut bit = mode + 1;
    let has_ib2 = info.ab != 0 && info.ib2 != 0;
    let cw = bc7_weights(info.ib);
    let aw = bc7_weights(if has_ib2 { info.ib2 } else { info.ib });
    let partition = get_bits(block, bit, info.pb) as usize;
    bit += info.pb;
    let rotation = get_bits(block, bit, info.rb);
    bit += info.rb;
    let index_sel = get_bits(block, bit, info.isb);
    bit += info.isb;
    let endpoints = bc7_endpoints(block, info, &mut bit);
    let mut cibit = bit;
    let mut aibit = cibit + 16 * info.ib - info.ns;
    for (i, px) in col.iter_mut().enumerate() {
        let s = bc7_subset(info.ns, partition, i) << 1;
        let mut ib = info.ib;
        let anchor = |table: &[u32; 64]| pick(table, partition) as usize == i;
        if i == 0 {
            ib -= 1;
        } else if info.ns == 2 {
            if anchor(&BC7_AI0) {
                ib -= 1;
            }
        } else if info.ns == 3 && (anchor(&BC7_AI1) || anchor(&BC7_AI2)) {
            ib -= 1;
        }
        let i0 = get_bits(block, cibit, ib) as usize;
        cibit += ib;
        let (e0, e1) = (pick(&endpoints, s), pick(&endpoints, s + 1));
        let mut c = if has_ib2 {
            let ib2 = if i == 0 { info.ib2 - 1 } else { info.ib2 };
            let i1 = get_bits(block, aibit, ib2) as usize;
            aibit += ib2;
            if index_sel != 0 {
                bc7_lerp(e0, e1, pick(aw, i1), pick(cw, i0))
            } else {
                bc7_lerp(e0, e1, pick(cw, i0), pick(aw, i1))
            }
        } else {
            bc7_lerp(e0, e1, pick(cw, i0), pick(cw, i0))
        };
        match rotation {
            1 => c.swap(0, 3),
            2 => c.swap(1, 3),
            3 => c.swap(2, 3),
            _ => {}
        }
        *px = c;
    }
}
