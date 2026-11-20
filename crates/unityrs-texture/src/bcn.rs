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
//! ([`put_block_data`]), a block's row of pixels at a time.
//!
//! **For speed**, each block's colours are computed once into a palette
//! (BC1's four, the alpha block's eight, BC7's per subset and index), and
//! the pixels are the palette's entries, each a `u32` of its four bytes;
//! the arithmetic per entry is Pillow's.

use crate::place::put_block_data;

mod bc7;
use bc7::bc7_block;

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

/// The `i`th entry of `table`, or the default where it has none (never, for
/// the indices the decoders compute).
fn pick<T: Copy + Default>(table: &[T], i: usize) -> T {
    table.get(i).copied().unwrap_or_default()
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
    put_block_data(
        out,
        w.saturating_mul(N / 4),
        h,
        (w.div_ceil(4), h.div_ceil(4)),
        data.as_chunks::<B>().0,
        #[inline(always)]
        |src, rows| {
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
    put_rgba::<0, 1>(rows, &palette, colour >> 32, &[0], 0);
}

/// `decode_bc2_block`, into the block's rows.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc2_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let (alpha, colour) = halves(src);
    let palette = bc1_palette4(colour);
    put_rgba::<4, 16>(rows, &palette, colour >> 32, &BC2_ALPHA, alpha);
}

/// BC3's colour and alpha blocks, into the block's rows.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc3_block(src: &[u8; 16], rows: [&mut [u8; 16]; 4]) {
    let (alpha, colour) = halves(src);
    let palette = bc1_palette4(colour);
    let values = bc3_palette::<24>(alpha);
    put_rgba::<3, 8>(rows, &palette, colour >> 32, &values, alpha >> 16);
}

/// `decode_bc4_block`, into the block's rows of `L`.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc4_block(src: &[u8; 8], rows: [&mut [u8; 4]; 4]) {
    let block = u64::from_le_bytes(*src);
    let values = bc3_palette::<0>(block);
    for (y, row) in rows.into_iter().enumerate() {
        let [a, b, c, d] = [0, 1, 2, 3].map(|x| pick(&values, alpha_index(block, 4 * y + x)));
        *row = (a | b << 8 | c << 16 | d << 24).to_le_bytes();
    }
}

/// `decode_bc5_block`, into the block's rows of `RGB`: blue is the 0 of
/// Pillow's zeroed block.
#[allow(clippy::inline_always, reason = "measured: every block's decode")]
#[inline(always)]
fn bc5_block(src: &[u8; 16], rows: [&mut [u8; 12]; 4]) {
    let (red, green) = halves(src);
    let (reds, greens) = (bc3_palette::<0>(red), bc3_palette::<8>(green));
    for (y, row) in rows.into_iter().enumerate() {
        let mut v = 0u128;
        for x in 0..4 {
            let n = 4 * y + x;
            let px = pick(&reds, alpha_index(red, n)) | pick(&greens, alpha_index(green, n));
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
#[allow(clippy::inline_always, reason = "measured: every block's pixels")]
#[inline(always)]
fn put_rgba<const AB: u32, const A: usize>(
    rows: [&mut [u8; 16]; 4],
    palette: &[u32; 4],
    lut: u64,
    alpha: &[u32; A],
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

/// `decode_bc1_color`'s four-colour form, as BC2 and BC3 always decode
/// it (`separate_alpha`), alpha 0: its thirds a channel at a time, each
/// `x * 683 >> 11`, exact for the sums to 2047 (`alpha_divisions_are_exact`).
#[inline]
fn bc1_palette4(block: u64) -> [u32; 4] {
    #[allow(clippy::cast_possible_truncation, reason = "the block's fields")]
    let (c0, c1) = (u32::from(block as u16), u32::from((block >> 16) as u16));
    let ([r0, g0, b0, _], [r1, g1, b1, _]) = (decode_565(c0), decode_565(c1));
    let third = |a: u32, b: u32| ((2 * a + b) * 683) >> 11;
    let rgb = |r: u32, g: u32, b: u32| r | g << 8 | b << 16;
    [
        rgb(r0, g0, b0),
        rgb(r1, g1, b1),
        rgb(third(r0, r1), third(g0, g1), third(b0, b1)),
        rgb(third(r1, r0), third(g1, g0), third(b1, b0)),
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
/// them. Pillow's two forms by the endpoints' order (a branch: real
/// blocks keep to one form, so it is predicted): entry `i` is `(wa a0 +
/// wb a1) / d`, so the endpoints are themselves, with `d`'s division a
/// multiply by `ceil(2^16 / d)` kept high, exact for the sums up to
/// `255 d` (`alpha_divisions_are_exact`), and the six-value form's last
/// two its 0 and 255. Each value is shifted `S` bits up, to its channel.
#[inline]
fn bc3_palette<const S: u32>(block: u64) -> [u32; 8] {
    #[allow(clippy::cast_possible_truncation, reason = "the block's bytes")]
    let (a0, a1) = (u32::from(block as u8), u32::from((block >> 8) as u8));
    // The products `(wa a0 + wb a1) m` in turn from the first weights
    // `(d - 1, 1)`, each the last plus `(a1 - a0) m`: the same integers
    // (each below 2^25; the sum wraps where the step is negative), and
    // each quotient at most 255.
    let ramp = |m: u32, d: u32| {
        let step = a1.wrapping_sub(a0).wrapping_mul(m);
        let mut product = ((d - 1) * a0 + a1) * m;
        let mut out = [0; 6];
        for o in &mut out {
            *o = (product >> 16) << S;
            product = product.wrapping_add(step);
        }
        out
    };
    let (e0, e1) = (a0 << S, a1 << S);
    if a0 > a1 {
        let [v2, v3, v4, v5, v6, v7] = ramp(u32::from(ALPHA_DIV[1]), 7);
        [e0, e1, v2, v3, v4, v5, v6, v7]
    } else {
        let [v2, v3, v4, v5, ..] = ramp(u32::from(ALPHA_DIV[0]), 5);
        [e0, e1, v2, v3, v4, v5, 0, 0xff << S]
    }
}

/// `ceil(2^16 / 5)` and `ceil(2^16 / 7)`.
const ALPHA_DIV: [u16; 2] = [13108, 9363];

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
