// Ported from texture2ddecoder 1.0.5 (MIT): src/Texture2DDecoder/pvrtc.cpp and pvrtc.h, and color.h's copy_block_buffer. See THIRD_PARTY_LICENSES.
//! PVRTC1, both bit rates, including RGB names' alpha, punch-through colour,
//! wrapped neighbors, Morton order and the reference's small-image behavior.
//! Decode a fixed 3x3 neighborhood per block: no input-sized scratch.
//! The 2bpp missing weights only read explicitly stored checkerboard neighbors,
//! so the reference's in-place weight updates require no persistent table.
//!
//! **For speed**, a block row slides its neighborhood along: each block
//! decodes only its right-hand column of texels' colours, and only its own
//! weights (2bpp's missing ones also its four neighbors'). The bilinear
//! sums are the reference's, taken a row of texels at a time (each sum is
//! exact in integers, so their order does not matter), over the eight
//! colour channels of a texel at once, each an `i16` lane: a channel is at
//! most 31 and its weights' products sum to at most 32, so no sum leaves
//! the lane. A texel missing from the data reads as eight zero bytes,
//! which decode as the reference's default texel: colours and weights 0.

use crate::place::put_blocks;

/// A texel's two colours, A then B, each `[r, g, b, a]`, one lane each.
type Lanes = [u16; 8];

/// The bilinear factors of a pixel at `x` along a side of `size` pixels,
/// for the texels before, at and after its block.
#[allow(clippy::indexing_slicing, reason = "x is below S, the array's length")]
const fn axis<const S: usize>() -> [[u16; 3]; S] {
    let mut out = [[0; 3]; S];
    #[allow(clippy::cast_possible_truncation, reason = "a block side, 4 or 8")]
    let (size, half) = (S as u16, S as u16 / 2);
    let mut x = 0;
    while x < S {
        #[allow(clippy::cast_possible_truncation, reason = "below the side")]
        let xi = x as u16;
        out[x] = if xi <= half {
            [half - xi, half + xi, 0]
        } else {
            [0, size + half - xi, xi - half]
        };
        x += 1;
    }
    out
}

/// The texel's 8 bytes, little-endian: the modulation bits, then colours A
/// and B.
fn word(data: &[u8], n: usize) -> u64 {
    let at = n.saturating_mul(8);
    data.get(at..at.saturating_add(8))
        .and_then(|b| b.first_chunk::<8>())
        .map_or(0, |b| u64::from_le_bytes(*b))
}

/// A colour's 5-bit channels (alpha 4-bit), opaque or translucent; `a` is
/// colour A, whose blue has a bit less. Both forms are computed and the
/// colour's kept, without a branch on its top bit.
fn colour(c: u16, a: bool) -> [u16; 4] {
    let opaque = [
        (c >> 10) & 31,
        (c >> 5) & 31,
        if a {
            (c & 0x1e) | ((c >> 4) & 1)
        } else {
            c & 31
        },
        15,
    ];
    let translucent = [
        ((c >> 7) & 0x1e) | ((c >> 11) & 1),
        ((c >> 3) & 0x1e) | ((c >> 7) & 1),
        if a {
            ((c << 1) & 0x1c) | ((c >> 2) & 3)
        } else {
            ((c << 1) & 0x1e) | ((c >> 3) & 1)
        },
        (c >> 11) & 0x0e,
    ];
    std::hint::select_unpredictable(c & 0x8000 != 0, opaque, translucent)
}

/// A texel's colours as lanes.
fn colours(word: u64) -> Lanes {
    #[allow(clippy::cast_possible_truncation, reason = "the texel's fields")]
    let (ca, cb) = ((word >> 32) as u16, (word >> 48) as u16);
    let [[r, g, b, a], [r2, g2, b2, a2]] = [colour(ca, true), colour(cb, false)];
    [r, g, b, a, r2, g2, b2, a2]
}

/// A texel's weights, pixel by pixel, a missing 2bpp weight negative (-1
/// vertical, -2 horizontal, -3 all four neighbors), and its punch-through
/// pixels, one bit each.
fn modulation(word: u64, is2: bool) -> ([i8; 32], u32) {
    let [m0, _, m2, _, a0, ..] = word.to_le_bytes();
    let mut weights = [0; 32];
    let mut punch = 0;
    #[allow(clippy::cast_possible_truncation, reason = "the low half")]
    let mut bits = word as u32;
    let mode = a0 & 1 != 0;
    let pick = |t: &[i8; 4], i: u32| t.get(i as usize).copied().unwrap_or(0);
    for (i, weight) in weights
        .iter_mut()
        .enumerate()
        .take(if is2 { 32 } else { 16 })
    {
        if !is2 {
            let index = bits & 3;
            *weight = pick(if mode { &[0, 4, 4, 8] } else { &[0, 3, 5, 8] }, index);
            if mode && index == 2 {
                punch |= 1 << i;
            }
            bits >>= 2;
        } else if !mode {
            *weight = if bits & 1 != 0 { 8 } else { 0 };
            bits >>= 1;
        } else if (i % 8 + i / 8) % 2 == 0 {
            *weight = pick(&[0, 3, 5, 8], bits & 3);
            if i == 0 || (i == 20 && m0 & 1 != 0) {
                *weight = (*weight + 3) & 8;
            }
            bits >>= 2;
        } else {
            *weight = if m0 & 1 == 0 {
                -3
            } else if m2 & 16 != 0 {
                -1
            } else {
                -2
            };
        }
    }
    (weights, punch)
}

/// The 2bpp missing weights, from the stored ones of the block and of its
/// neighbors above, left, right and below (`near`'s words).
fn fill_missing(weights: &mut [i8; 32], near: [u64; 4]) {
    let [up, left, right, down] = near.map(|w| modulation(w, true).0);
    let own = *weights;
    let read = |t: &[i8; 32], i: usize| i32::from(t.get(i).copied().unwrap_or(0));
    for (i, w) in weights.iter_mut().enumerate() {
        if *w >= 0 {
            continue;
        }
        let (x, y) = (i % 8, i / 8);
        let up = if y == 0 {
            read(&up, i + 24)
        } else {
            read(&own, i - 8)
        };
        let down = if y == 3 {
            read(&down, i - 24)
        } else {
            read(&own, i + 8)
        };
        let left = if x == 0 {
            read(&left, i + 7)
        } else {
            read(&own, i - 1)
        };
        let right = if x == 7 {
            read(&right, i - 7)
        } else {
            read(&own, i + 1)
        };
        let v = match *w {
            -1 => (up + down + 1) / 2,
            -2 => (left + right + 1) / 2,
            _ => (up + down + left + right + 2) / 4,
        };
        // Stored weights are 0 to 8, and so their averages.
        *w = i8::try_from(v).unwrap_or(0);
    }
}

/// Interleave y then x up to the smaller dimension, then append the long
/// side: the bits a texel's column (`odd`, at the odd interleaved bits) or
/// row contributes, which together `|` to its index.
fn morton_part(v: usize, min: usize, odd: bool) -> usize {
    let (mut offset, mut shift, mut mask) = (0, 0, 1);
    while mask < min {
        offset |= ((v & mask) << usize::from(odd)) << shift;
        shift += 1;
        mask <<= 1;
    }
    offset | ((v >> shift) << (shift * 2))
}

/// The 3x3 neighborhood's texels of a block row, as it slides: by column
/// (left, centre, right), each by row (above, at, below) its word and
/// colours.
#[derive(Clone, Copy, Default)]
struct Column {
    words: [u64; 3],
    colours: [Lanes; 3],
}

/// Sizes are padded as UnityPy pads them when finding the neighborhood;
/// the final store crops and flips to the original size.
pub(crate) fn decode(data: &[u8], w: usize, h: usize, is2: bool, out: &mut [u8]) {
    if is2 {
        blocks::<8, 32>(data, w, h, out);
    } else {
        blocks::<4, 16>(data, w, h, out);
    }
}

/// [`decode`] for blocks `BW` pixels wide, `N = 4 BW` bytes a block row.
#[allow(
    clippy::inline_always,
    reason = "measured: each block inlined into the block loop"
)]
fn blocks<const BW: usize, const N: usize>(data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let (nx, ny) = (w.div_ceil(BW), h.div_ceil(4));
    if nx == 0 || ny == 0 {
        return;
    }
    let min = nx.min(ny);
    let mut rows_at = [0; 3];
    let mut window = [Column::default(); 3];
    let column = |x: usize, rows_at: &[usize; 3]| {
        let mx = morton_part(x, min, true);
        let words = rows_at.map(|my| word(data, mx | my));
        Column {
            words,
            colours: words.map(colours),
        }
    };
    put_blocks::<N>(
        out,
        w.saturating_mul(4),
        h,
        (nx, ny),
        #[inline(always)]
        |bx, by, rows| {
            if bx == 0 {
                rows_at =
                    [(by + ny - 1) % ny, by, (by + 1) % ny].map(|y| morton_part(y, min, false));
                window = [(nx - 1) % nx, 0, 1 % nx].map(|x| column(x, &rows_at));
            } else {
                let [_, centre, right] = window;
                window = [centre, right, column((bx + 1) % nx, &rows_at)];
            }
            block::<BW, N>(&window, rows);
            true
        },
    );
}

/// The weights of a block's pixels, 0 to 8, and its punch-through pixels,
/// one bit each: 4bpp's without a branch on the texel's mode, 2bpp's
/// missing ones filled from the neighbors.
fn weights<const BW: usize>(window: &[Column; 3]) -> ([u16; 32], u32) {
    let [left, centre, right] = window;
    let [up, word, down] = centre.words;
    let mut out = [0; 32];
    if BW == 4 {
        // By the mode bit (bit 0 of colour A).
        const TABLE: [[u16; 4]; 2] = [[0, 3, 5, 8], [0, 4, 4, 8]];
        let mode = usize::from(word >> 32 & 1 != 0);
        let table = TABLE.get(mode).copied().unwrap_or_default();
        let mut punch = 0;
        for (i, w) in out.iter_mut().enumerate().take(16) {
            #[allow(clippy::cast_possible_truncation, reason = "masked to 2 bits")]
            let index = (word >> (2 * i) & 3) as usize;
            *w = table.get(index).copied().unwrap_or(0);
            punch |= u32::from(mode != 0 && index == 2) << i;
        }
        return (out, punch);
    }
    let (mut weights, punch) = modulation(word, true);
    if weights.iter().any(|&w| w < 0) {
        fill_missing(&mut weights, [up, left.words[1], right.words[1], down]);
    }
    for (o, w) in out.iter_mut().zip(weights) {
        *o = u16::try_from(w).unwrap_or(0);
    }
    (out, punch)
}

/// One block's pixels from its neighborhood, into its rows of `[r, g, b,
/// a]`.
fn block<const BW: usize, const N: usize>(window: &[Column; 3], rows: [&mut [u8; N]; 4]) {
    let wx = axis::<BW>();
    let wy = axis::<4>();
    let [left, centre, right] = window;
    // Each texel row's sum along x, pixel column by pixel column.
    let mut across = [[[0u16; 8]; BW]; 3];
    for (j, sums) in across.iter_mut().enumerate() {
        let at = |c: &Column| c.colours.get(j).copied().unwrap_or_default();
        let (l, c, r) = (at(left), at(centre), at(right));
        for (sum, &[fl, fc, fr]) in sums.iter_mut().zip(&wx) {
            *sum = mac(mac(mul(l, fl), fc, c), fr, r);
        }
    }
    let (weights, punch) = weights::<BW>(window);
    let [above, at, below] = &across;
    for (y, (row, &[fa, fb, fc])) in rows.into_iter().zip(&wy).enumerate() {
        let pixels = row.as_chunks_mut::<4>().0.iter_mut();
        let sums = above.iter().zip(at).zip(below);
        for (x, (dst, ((a, b), c))) in pixels.zip(sums).enumerate() {
            let i = y * BW + x;
            let sum = mac(mac(mul(*a, fa), fb, *b), fc, *c);
            let m = weights.get(i).copied().unwrap_or(0);
            let mut px = blend(scale::<BW>(sum), m);
            if punch >> i & 1 != 0 {
                px[3] = 0;
            }
            *dst = px;
        }
    }
}

/// `v` times `f`, lane by lane.
#[inline]
fn mul(v: Lanes, f: u16) -> Lanes {
    let [v0, v1, v2, v3, v4, v5, v6, v7] = v;
    [
        v0 * f,
        v1 * f,
        v2 * f,
        v3 * f,
        v4 * f,
        v5 * f,
        v6 * f,
        v7 * f,
    ]
}

/// `acc + f v`, lane by lane.
#[inline]
fn mac(acc: Lanes, f: u16, v: Lanes) -> Lanes {
    let [a0, a1, a2, a3, a4, a5, a6, a7] = acc;
    let [v0, v1, v2, v3, v4, v5, v6, v7] = mul(v, f);
    [
        a0 + v0,
        a1 + v1,
        a2 + v2,
        a3 + v3,
        a4 + v4,
        a5 + v5,
        a6 + v6,
        a7 + v7,
    ]
}

/// The channels' bilinear sums to 8 bits, as the reference scales them:
/// both rules for every lane, each lane keeping its own.
#[inline]
fn scale<const BW: usize>(c: Lanes) -> Lanes {
    // `(c >> s1) + (c >> s2)`, lane by lane.
    let two = |s1: u32, s2: u32| {
        let [c0, c1, c2, c3, c4, c5, c6, c7] = c;
        [
            (c0 >> s1) + (c0 >> s2),
            (c1 >> s1) + (c1 >> s2),
            (c2 >> s1) + (c2 >> s2),
            (c3 >> s1) + (c3 >> s2),
            (c4 >> s1) + (c4 >> s2),
            (c5 >> s1) + (c5 >> s2),
            (c6 >> s1) + (c6 >> s2),
            (c7 >> s1) + (c7 >> s2),
        ]
    };
    // The alpha rule's `c + (c >> 4)` is `(c >> 0) + (c >> 4)`.
    let ([r0, g0, b0, _, r1, g1, b1, _], [_, _, _, a0, _, _, _, a1]) = if BW == 8 {
        (two(2, 7), two(1, 5))
    } else {
        (two(1, 6), two(0, 4))
    };
    [r0, g0, b0, a0, r1, g1, b1, a1]
}

/// `(a (8 - m) + b m) / 8` for colours A and B's scaled channels, as
/// bytes: each channel is at most 255 and `m` at most 8, so in a `u16`,
/// and so is the result, under 256.
#[inline]
fn blend(c: Lanes, m: u16) -> [u8; 4] {
    let [a0, a1, a2, a3, b0, b1, b2, b3] = c;
    let n = 8_u16.saturating_sub(m);
    [
        a0 * n + b0 * m,
        a1 * n + b1 * m,
        a2 * n + b2 * m,
        a3 * n + b3 * m,
    ]
    .map(|v| u8::try_from(v / 8).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    /// The reference's Morton index, interleaving y then x up to the
    /// smaller dimension and appending the long side.
    fn morton(x: usize, y: usize, min: usize) -> usize {
        let (mut offset, mut shift, mut mask) = (0, 0, 1);
        while mask < min {
            offset |= ((y & mask) | ((x & mask) << 1)) << shift;
            shift += 1;
            mask <<= 1;
        }
        offset | (((x | y) >> shift) << (shift * 2))
    }

    /// A texel's index is its column's part and its row's, joined.
    #[test]
    fn morton_parts_join_to_the_reference_index() {
        for (nx, ny) in [(1, 1), (1, 8), (8, 1), (4, 4), (16, 2), (2, 64), (32, 32)] {
            let min = usize::min(nx, ny);
            for x in 0..nx {
                for y in 0..ny {
                    let joined =
                        super::morton_part(x, min, true) | super::morton_part(y, min, false);
                    assert_eq!(joined, morton(x, y, min), "{x} {y} of {nx}x{ny}");
                }
            }
        }
    }
}
