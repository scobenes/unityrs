// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/BcnDecode.c, its BC1, BC2, BC3, BC4, BC5 and BC7 block decoders and its block order (decode_bc1_color, decode_bc3_alpha, decode_bc2_block, decode_bc4_block, decode_bc5_block, decode_bc7_block, put_block, decode_bcn); that file is dedicated to the public domain (CC0) in its own header, and Pillow's licence covers the rest. See THIRD_PARTY_LICENSES.
//! Pillow 9.5.0's `BCn` decoders for DXT1 (`bcn` 1), DXT3 (`bcn` 2), DXT5
//! (`bcn` 3), BC4 (`bcn` 4), BC5 (`bcn` 5) and BC7 (`bcn` 7), which UnityPy
//! 1.25.2 calls through `Image.frombytes` in mode `RGBA`, but `L` for BC4
//! and `RGB` for BC5 (`docs/plans/extract.md` E15; `CONV_TABLE`), ported
//! rule for rule:
//! - **BC1** (`decode_bc1_color`): the two 5:6:5 endpoints widened by bit
//!   replication; four colours when `c0 > c1` (the thirds, each `(2a + b) / 3`
//!   truncated), else three and transparent black (the half, truncated, then
//!   (0, 0, 0, 0)). Alpha 255 elsewhere.
//! - **BC3**: the colour block as BC1's, always in its four-colour form
//!   (`separate_alpha`), and the alpha block (`decode_bc3_alpha`): eight
//!   values when `a0 > a1` (sevenths, truncated), else six with 0 and 255.
//! - **BC2** (`decode_bc2_block`): the colour block as BC1's, always in its
//!   four-colour form, and each pixel's alpha its own 4 bits, pixel `n` in
//!   bits `4n` to `4n + 3` of the first 8 bytes, widened as `(v << 4) | v`.
//! - **BC4** (`decode_bc4_block`): BC3's alpha block, unsigned, into the
//!   one band of `L`.
//! - **BC5** (`decode_bc5_block`): two of BC3's alpha blocks, unsigned
//!   (UnityPy passes no pixel format, so never `BC5S`), into red and green;
//!   blue is the 0 of Pillow's zeroed block (`memset`), as is the alpha the
//!   `RGB` image drops.
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
//! pixels that fall inside the texture straight to their flipped rows
//! ([`put_blocks`]), a block's row of pixels at a time.
//!
//! **For speed**, each block's colours are computed once into a palette
//! (BC1's four, the alpha block's eight, BC7's per subset and index), and
//! the pixels are the palette's entries, each a `u32` of its four bytes;
//! the arithmetic per entry is Pillow's.

use crate::place::put_blocks;

/// Which of Pillow's decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `bcn` 1: DXT1.
    Bc1,
    /// `bcn` 2: DXT3.
    Bc2,
    /// `bcn` 3: DXT5.
    Bc3,
    /// `bcn` 4: BC4, into `L`.
    Bc4,
    /// `bcn` 5: BC5, into `RGB`.
    Bc5,
    /// `bcn` 7: BC7.
    Bc7,
}

/// A block's 16 pixels, each `u32::from_le_bytes([r, g, b, a])`.
type Pixels = [u32; 16];

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
/// `w` x `h` image's rows top to bottom, each pixel the first bytes of the
/// block's pixel its mode keeps (4 for `RGBA`, 3 for BC5's `RGB`, 1 for
/// BC4's `L`): block row `by` of the padded image holds source rows `4 by`
/// to `4 by + 3`, and source row `y` is output row `h - 1 - y`.
// Each block decoder through an inlined closure: passed itself, its `Fn`
// shim was a call per block (measured).
#[allow(
    clippy::inline_always,
    clippy::redundant_closure,
    reason = "measured: every block's decode inlined"
)]
pub(crate) fn decode(kind: Kind, data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let size = (w, h);
    match kind {
        Kind::Bc1 => blocks::<8, 16>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc1_block(s, r),
        ),
        Kind::Bc2 => blocks::<16, 16>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc2_block(s, r),
        ),
        Kind::Bc3 => blocks::<16, 16>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc3_block(s, r),
        ),
        Kind::Bc4 => blocks::<8, 4>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc4_block(s, r),
        ),
        Kind::Bc5 => blocks::<16, 12>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc5_block(s, r),
        ),
        Kind::Bc7 => blocks::<16, 16>(
            data,
            size,
            out,
            #[inline(always)]
            |s, r| bc7_block(s, r),
        ),
    }
}

/// [`decode`] for blocks of `B` bytes, each of whose rows of 4 pixels is
/// `N` bytes, `block` writing a block's rows.
#[allow(
    clippy::inline_always,
    reason = "measured: each block's decode inlined into the block loop"
)]
#[inline(always)]
fn blocks<const B: usize, const N: usize>(
    data: &[u8],
    (w, h): (usize, usize),
    out: &mut [u8],
    block: impl Fn(&[u8; B], [&mut [u8; N]; 4]),
) {
    let mut blocks = data.as_chunks::<B>().0.iter();
    put_blocks(
        out,
        w.saturating_mul(N / 4),
        h,
        (w.div_ceil(4), h.div_ceil(4)),
        #[inline(always)]
        |_, _, rows| {
            let Some(src) = blocks.next() else {
                return false;
            };
            block(src, rows);
            true
        },
    );
}

/// `decode_bc1_color` as DXT1 decodes it, into the block's rows.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc1_block(src: &[u8; 8], rows: [&mut [u8; 16]; 4]) {
    let colour = u64::from_le_bytes(*src);
    let palette = bc1_palette(colour, false);
    put_rgba::<0>(rows, &palette, colour >> 32, &[0; 16], 0);
}

/// `decode_bc2_block`, into the block's rows.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc2_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let (alpha, colour) = halves(src);
    let palette = bc1_palette(colour, true).map(|p| p & 0x00FF_FFFF);
    put_rgba::<4>(rows, &palette, colour >> 32, &BC2_ALPHA, alpha);
}

/// BC3's colour and alpha blocks, into the block's rows.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc3_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let (alpha, colour) = halves(src);
    let palette = bc1_palette(colour, true).map(|p| p & 0x00FF_FFFF);
    let mut values = [0; 16];
    for (v, a) in values.iter_mut().zip(bc3_palette(alpha)) {
        *v = u32::from(a) << 24;
    }
    put_rgba::<3>(rows, &palette, colour >> 32, &values, alpha >> 16);
}

/// `decode_bc4_block`, into the block's rows of `L`.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc4_block(src: &[u8; 8], rows: [&mut [u8; 4]; 4]) {
    let block = u64::from_le_bytes(*src);
    let values = bc3_palette(block);
    for (y, row) in rows.into_iter().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = pick(&values, alpha_index(block, 4 * y + x));
        }
    }
}

/// `decode_bc5_block`, into the block's rows of `RGB`: blue is the 0 of
/// Pillow's zeroed block.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc5_block(src: &[u8; 16], rows: [&mut [u8; 12]; 4]) {
    let (red, green) = halves(src);
    let (reds, greens) = (bc3_palette(red), bc3_palette(green));
    for (y, row) in rows.into_iter().enumerate() {
        let mut v = 0u128;
        for x in 0..4 {
            let n = 4 * y + x;
            let px = u32::from(pick(&reds, alpha_index(red, n)))
                | u32::from(pick(&greens, alpha_index(green, n))) << 8;
            v |= u128::from(px) << (24 * x);
        }
        if let Some(bytes) = v.to_le_bytes().first_chunk::<12>() {
            *row = *bytes;
        }
    }
}

/// Pixel `n`'s 3-bit index in an alpha block: bits `16 + 3n` on.
#[inline]
fn alpha_index(block: u64, n: usize) -> usize {
    #[allow(clippy::cast_possible_truncation, reason = "masked to 3 bits")]
    let i = (block >> (16 + 3 * n) & 7) as usize;
    i
}

/// A 16-byte block's two halves, each a little-endian `u64`.
fn halves(b: &[u8; 16]) -> (u64, u64) {
    let v = u128::from_le_bytes(*b);
    #[allow(clippy::cast_possible_truncation, reason = "the low half")]
    (v as u64, (v >> 64) as u64)
}

/// `decode_bc2_block`'s alphas by their 4 bits `av`, `(av << 4) | av`,
/// in the alpha byte.
#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time, av below 16"
)]
const BC2_ALPHA: [u32; 16] = {
    let mut a = [0; 16];
    let mut av: u32 = 0;
    while av < 16 {
        a[av as usize] = (av << 4 | av) << 24;
        av += 1;
    }
    a
};

/// Writes a block's pixels into its rows of `[r, g, b, a]`: pixel `n` is
/// the union of `palette`'s entry by its 2-bit index (bits `2n` of `lut`)
/// and `alpha`'s by its `AB`-bit one (bits `AB n` of `alut`).
#[inline]
fn put_rgba<const AB: u32>(
    rows: [&mut [u8; 16]; 4],
    palette: &[u32; 4],
    lut: u64,
    alpha: &[u32; 16],
    alut: u64,
) {
    for (r, row) in rows.into_iter().enumerate() {
        for (x, dst) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            // Each index from the block itself, none waiting on the last.
            let n = 4 * r + x;
            let (i, ai) = (
                (lut >> (2 * n)) & 3,
                (alut >> (AB as usize * n)) & ((1 << AB) - 1),
            );
            #[allow(clippy::cast_possible_truncation, reason = "masked indices")]
            let p = pick(palette, i as usize) | if AB == 0 { 0 } else { pick(alpha, ai as usize) };
            *dst = p.to_le_bytes();
        }
    }
}

/// `decode_565`, as a pixel of alpha 255.
fn decode_565(x: u32) -> [u32; 4] {
    let mut r = (x & 0xf800) >> 8;
    r |= r >> 5;
    let mut g = (x & 0x7e0) >> 3;
    g |= g >> 6;
    let mut b = (x & 0x1f) << 3;
    b |= b >> 5;
    [r, g, b, 0xff]
}

/// `decode_bc1_color`'s four colours: `block` is the 8-byte colour block,
/// whose indices (its high 32 bits) pick them. The thirds, each
/// `(2a + b) / 3`, and the half `(a + b) / 2` are computed for the three
/// channels at once, each in a 21-bit lane of a `u64` ([`lanes`]): a sum
/// is at most 765, and `x * 683 >> 11` is `x / 3` for every `x` to 2047,
/// so no lane's product reaches the next; each result is its lane's low
/// byte. The form, four colours or three and transparent black, is chosen
/// without a branch.
#[inline]
fn bc1_palette(block: u64, separate_alpha: bool) -> [u32; 4] {
    #[allow(clippy::cast_possible_truncation, reason = "the block's fields")]
    let (c0, c1) = (u32::from(block as u16), u32::from((block >> 16) as u16));
    let (l0, l1) = (lanes(decode_565(c0)), lanes(decode_565(c1)));
    let third = |v: u64| unlanes((v * 683) >> 11);
    let (t2, t3) = (third(2 * l0 + l1), third(l0 + 2 * l1));
    // Lane by lane: no lane's sum reaches the next.
    #[allow(clippy::manual_midpoint, reason = "the lanes' sums, under 2^10")]
    let half = unlanes((l0 + l1) >> 1);
    let four = u32::from(c0 > c1 || separate_alpha).wrapping_neg();
    [
        unlanes(l0),
        unlanes(l1),
        (t2 & four) | (half & !four),
        t3 & four,
    ]
}

/// A pixel's red, green and blue in the 21-bit lanes of a `u64`.
fn lanes([r, g, b, _]: [u32; 4]) -> u64 {
    u64::from(r) | u64::from(g) << 21 | u64::from(b) << 42
}

/// The pixel of [`lanes`]' low bytes, of alpha 255: each byte moved down
/// to its place.
fn unlanes(v: u64) -> u32 {
    #[allow(clippy::cast_possible_truncation, reason = "masked to 3 bytes")]
    let rgb = ((v & 0xFF) | (v >> 13 & 0xFF00) | (v >> 26 & 0xFF_0000)) as u32;
    rgb | 0xFF00_0000
}

/// `decode_bc3_alpha`'s eight values, unsigned: `block` is the 8-byte
/// alpha block, whose 3-bit indices, pixel `n`'s from bit `16 + 3n`, pick
/// them. Pillow's two forms are one computation over eight lanes, its
/// constants chosen by the endpoints' order (no branch on it): entry `i`
/// is `(wa a0 + wb a1) / d`, so the endpoints are themselves, with `d`'s
/// division a multiply by `ceil(2^16 / d)` kept high, exact for the sums
/// up to `255 d` (`alpha_divisions_are_exact`), and the six-value form's
/// last two its 0 and 255.
#[inline]
fn bc3_palette(block: u64) -> [u8; 8] {
    // By `a0 > a1`: the six-value form, then the eight-value one.
    const WA: [[u16; 8]; 2] = [[5, 0, 4, 3, 2, 1, 0, 0], [7, 0, 6, 5, 4, 3, 2, 1]];
    const WB: [[u16; 8]; 2] = [[0, 5, 1, 2, 3, 4, 0, 0], [0, 7, 1, 2, 3, 4, 5, 6]];
    const ENDS: [[u16; 8]; 2] = [[0, 0, 0, 0, 0, 0, 0, 0xff], [0; 8]];
    const M: [u16; 2] = [ALPHA_DIV[0], ALPHA_DIV[1]];
    let [a0, a1, ..] = block.to_le_bytes().map(u16::from);
    let sel = usize::from(a0 > a1);
    let (wa, wb, ends, m) = (
        pick(&WA, sel),
        pick(&WB, sel),
        pick(&ENDS, sel),
        pick(&M, sel),
    );
    let mut out = [0; 8];
    for (o, ((wa, wb), end)) in out.iter_mut().zip(wa.iter().zip(wb).zip(ends)) {
        // At most `255 d`.
        let sum = wa * a0 + wb * a1;
        *o = byte((u32::from(sum) * u32::from(m)) >> 16 | u32::from(end));
    }
    out
}

/// `ceil(2^16 / 5)` and `ceil(2^16 / 7)`.
const ALPHA_DIV: [u16; 2] = [13108, 9363];

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

/// `bc7_lerp` at weight `s` of 64, `((64 - s) e0 + s e1 + 32) >> 6`, for
/// the channels `keep` holds the bytes of, the rest 0: the four channels
/// at once, each in a `u16` (each endpoint under 256, so the sum under
/// 2^14).
fn lerp_pixel(e0: [u16; 4], e1: [u16; 4], s: u32, keep: u32) -> u32 {
    let s = u16::try_from(s).unwrap_or(0);
    let t = 64 - s;
    let [a0, a1, a2, a3] = e0;
    let [b0, b1, b2, b3] = e1;
    let v = [
        (t * a0 + s * b0 + 32) >> 6,
        (t * a1 + s * b1 + 32) >> 6,
        (t * a2 + s * b2 + 32) >> 6,
        (t * a3 + s * b3 + 32) >> 6,
    ];
    u32::from_le_bytes(v.map(|c| byte(u32::from(c)))) & keep
}

/// An endpoint's channels, each under 256, as [`lerp_pixel`] takes them.
fn narrow(e: [u32; 4]) -> [u16; 4] {
    e.map(|c| u16::from(byte(c)))
}

/// The six endpoints a BC7 block holds at most (three subsets), each
/// `[r, g, b, a]`.
type Endpoints = [[u32; 4]; 6];

/// `decode_bc7_block`'s endpoints: read from bit `*bit` (moved past them),
/// channel by channel, then the p-bits, then each widened to 8 bits.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
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

/// The pixels of `bc7_block`'s modes 4 and 5 (one subset, two index sets,
/// rotation and index selection): their two palettes, one per index set,
/// each holding the channels its set weights, rotated, so a pixel is the
/// two entries' union.
struct Split {
    first: [u32; 16],
    second: [u32; 16],
}

/// `decode_bc7_block`, into the block's rows of `[r, g, b, a]`.
fn bc7_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let px = bc7_pixels(src);
    for (row, px) in rows.into_iter().zip(px.as_chunks::<4>().0) {
        for (dst, p) in row.as_chunks_mut::<4>().0.iter_mut().zip(px) {
            *dst = p.to_le_bytes();
        }
    }
}

/// `decode_bc7_block`'s pixels.
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
    let mut ci = indices(bit);
    if info.ab != 0 && info.ib2 != 0 {
        let mut ai = indices(bit + 16 * info.ib - info.ns);
        let split = bc7_split(info, &endpoints, rotation, index_sel);
        let (m0, m1) = ((1u64 << info.ib) - 1, (1u64 << info.ib2) - 1);
        let mut px = [0; 16];
        for (i, p) in px.iter_mut().enumerate() {
            // Pixel 0 alone is an anchor: one subset.
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
        let (e0, e1) = (pick(&endpoints, 2 * s), pick(&endpoints, 2 * s + 1));
        let (e0, e1) = (narrow(e0), narrow(e1));
        for (entry, &w) in entries.iter_mut().zip(cw) {
            *entry = lerp_pixel(e0, e1, w, u32::MAX);
        }
    }
    let (mut subsets, anchors, sb) = bc7_layout(info.ns, partition);
    let mut px = [0; 16];
    for (i, p) in px.iter_mut().enumerate() {
        let anchor = (anchors >> i) & 1;
        let nb = info.ib - anchor;
        #[allow(clippy::cast_possible_truncation, reason = "masked to 4 bits")]
        let k = (ci & ((1 << nb) - 1)) as usize;
        ci >>= nb;
        let s = (subsets & ((1 << sb) - 1)) as usize;
        subsets >>= sb;
        *p = pick(&palette, (s << 4 | k) & 63);
    }
    px
}

/// `bc7_block`'s modes 4 and 5: the palettes of their two index sets.
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_split(info: &ModeInfo, endpoints: &Endpoints, rotation: u32, index_sel: u32) -> Split {
    let [e0, e1, ..] = *endpoints;
    let (e0, e1) = (narrow(e0), narrow(e1));
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
        *entry = rotate(lerp_pixel(e0, e1, w, first_mask));
    }
    for (entry, &w) in split.second.iter_mut().zip(aw) {
        *entry = rotate(lerp_pixel(e0, e1, w, second_mask));
    }
    split
}

/// `bc7_get_subset` for every pixel of a partition, pixel `i`'s subset in
/// bits `i b` to `i b + b - 1`; the anchors, pixel `i`'s in bit `i`: pixel
/// 0 and the anchor of each further subset; and `b`, the bits a subset
/// takes (0, 1 or 2).
#[allow(clippy::inline_always, reason = "measured: the mode's widths folded")]
#[inline(always)]
fn bc7_layout(ns: u32, partition: usize) -> (u32, u32, u32) {
    match ns {
        2 => (
            pick(&BC7_SI2, partition),
            1 | 1 << pick(&BC7_AI0, partition),
            1,
        ),
        3 => (
            pick(&BC7_SI3, partition),
            1 | 1 << pick(&BC7_AI1, partition) | 1 << pick(&BC7_AI2, partition),
            2,
        ),
        _ => (0, 1, 0),
    }
}

#[cfg(test)]
mod tests {
    /// The divisions by multiplication are exact over every sum they take:
    /// [`super::bc1_palette`]'s thirds (to 765, in fact to 2047) and
    /// [`super::bc3_palette`]'s fifths and sevenths (to `255 d`).
    #[test]
    fn alpha_divisions_are_exact() {
        for x in 0..2048_u64 {
            assert_eq!((x * 683) >> 11, x / 3, "{x}");
        }
        for (d, m) in [(5_u16, super::ALPHA_DIV[0]), (7, super::ALPHA_DIV[1])] {
            for x in 0..=255 * u32::from(d) {
                assert_eq!((x * u32::from(m)) >> 16, x / u32::from(d), "{x} / {d}");
            }
        }
    }
}
