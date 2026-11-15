// Ported from texture2ddecoder 1.0.5 (MIT): src/Texture2DDecoder/pvrtc.cpp and pvrtc.h, and color.h's copy_block_buffer. See THIRD_PARTY_LICENSES.
//! PVRTC1, both bit rates, including RGB names' alpha, punch-through colour,
//! wrapped neighbors, Morton order and the reference's small-image behavior.
//! Decode a fixed 3x3 neighborhood anew per block: no input-sized scratch.
//! The 2bpp missing weights only read explicitly stored checkerboard neighbors,
//! so the reference's in-place weight updates require no persistent table.

#[derive(Clone, Copy, Default)]
struct Texel {
    colours: [[i32; 4]; 2],
    weights: [i32; 32],
    punch: u32,
}

fn at<T: Copy + Default, const N: usize>(values: &[T; N], i: usize) -> T {
    values.get(i).copied().unwrap_or_default()
}

/// Interleave y then x up to the smaller dimension, then append the long side.
fn morton(x: usize, y: usize, min: usize) -> usize {
    let (mut offset, mut shift, mut mask) = (0, 0, 1);
    while mask < min {
        offset |= ((y & mask) | ((x & mask) << 1)) << shift;
        shift += 1;
        mask <<= 1;
    }
    offset | (((x | y) >> shift) << (shift * 2))
}

fn texel(data: &[u8], n: usize, is2: bool) -> Texel {
    let Some(bytes) = data.get(n * 8..n * 8 + 8) else {
        return Texel::default();
    };
    let &[m0, m1, m2, m3, a0, a1, b0, b1] = bytes else {
        return Texel::default();
    };
    let (ca, cb) = (u16::from_le_bytes([a0, a1]), u16::from_le_bytes([b0, b1]));
    let colour = |c: u16, a: bool| -> [i32; 4] {
        let [r, g, b, alpha] = if c & 0x8000 != 0 {
            [
                (c >> 10) & 31,
                (c >> 5) & 31,
                if a {
                    (c & 0x1e) | ((c >> 4) & 1)
                } else {
                    c & 31
                },
                15,
            ]
        } else {
            [
                ((c >> 7) & 0x1e) | ((c >> 11) & 1),
                ((c >> 3) & 0x1e) | ((c >> 7) & 1),
                if a {
                    ((c << 1) & 0x1c) | ((c >> 2) & 3)
                } else {
                    ((c << 1) & 0x1e) | ((c >> 3) & 1)
                },
                (c >> 11) & 0x0e,
            ]
        };
        [r.into(), g.into(), b.into(), alpha.into()]
    };
    let mut t = Texel {
        colours: [colour(ca, true), colour(cb, false)],
        ..Texel::default()
    };
    let mut bits = u32::from_le_bytes([m0, m1, m2, m3]);
    let mode = a0 & 1 != 0;
    for (i, weight) in t
        .weights
        .iter_mut()
        .enumerate()
        .take(if is2 { 32 } else { 16 })
    {
        if !is2 {
            let index = (bits & 3) as usize;
            *weight = at(if mode { &[0, 4, 4, 8] } else { &[0, 3, 5, 8] }, index);
            if mode && index == 2 {
                t.punch |= 1 << i;
            }
            bits >>= 2;
        } else if !mode {
            *weight = if bits & 1 != 0 { 8 } else { 0 };
            bits >>= 1;
        } else if (i % 8 + i / 8) % 2 == 0 {
            *weight = at(&[0, 3, 5, 8], (bits & 3) as usize);
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
    t
}

fn axis(x: usize, size: usize) -> [i32; 3] {
    let (x, size) = (
        i32::try_from(x).unwrap_or(0),
        i32::try_from(size).unwrap_or(0),
    );
    let half = size / 2;
    if x <= half {
        [half - x, half + x, 0]
    } else {
        [0, size + half - x, x - half]
    }
}

fn weight(local: &[Texel; 9], x: usize, y: usize, bw: usize) -> i32 {
    let i = y * bw + x;
    let w = at(&at(local, 4).weights, i);
    if w >= 0 {
        return w;
    }
    let read = |block, index| at(&at(local, block).weights, index);
    let up = if y == 0 {
        read(1, i + 24)
    } else {
        read(4, i - 8)
    };
    let down = if y == 3 {
        read(7, i - 24)
    } else {
        read(4, i + 8)
    };
    let left = if x == 0 {
        read(3, i + 7)
    } else {
        read(4, i - 1)
    };
    let right = if x == 7 {
        read(5, i - 7)
    } else {
        read(4, i + 1)
    };
    match w {
        -1 => (up + down + 1) / 2,
        -2 => (left + right + 1) / 2,
        _ => (up + down + left + right + 2) / 4,
    }
}

fn pixel(local: &[Texel; 9], x: usize, y: usize, bw: usize) -> [u8; 4] {
    let (wx, wy) = (axis(x, bw), axis(y, 4));
    let mut colours = [[0i32; 4]; 2];
    for (n, t) in local.iter().enumerate() {
        let factor = at(&wx, n % 3) * at(&wy, n / 3);
        for (sum, endpoint) in colours.iter_mut().zip(t.colours) {
            for (s, c) in sum.iter_mut().zip(endpoint) {
                *s += factor * c;
            }
        }
    }
    for colour in &mut colours {
        for (ch, c) in colour.iter_mut().enumerate() {
            *c = if ch == 3 {
                if bw == 8 {
                    (*c >> 1) + (*c >> 5)
                } else {
                    *c + (*c >> 4)
                }
            } else if bw == 8 {
                (*c >> 2) + (*c >> 7)
            } else {
                (*c >> 1) + (*c >> 6)
            };
        }
    }
    let [endpoint_a, endpoint_b] = colours;
    let modulation = weight(local, x, y, bw);
    let mut out = [0; 4];
    for ((dst, a), b) in out.iter_mut().zip(endpoint_a).zip(endpoint_b) {
        *dst = u8::try_from((a * (8 - modulation) + b * modulation) / 8).unwrap_or(0);
    }
    if at(local, 4).punch & (1 << (y * bw + x)) != 0
        && let Some(alpha) = out.last_mut()
    {
        *alpha = 0;
    }
    out
}

/// Sizes are padded as UnityPy pads them when finding the neighborhood;
/// the final store crops and flips to the original size.
pub(crate) fn decode(data: &[u8], w: usize, h: usize, is2: bool, out: &mut [u8]) {
    let bw = if is2 { 8 } else { 4 };
    let (nx, ny) = (w.div_ceil(bw), h.div_ceil(4));
    for by in 0..ny {
        for bx in 0..nx {
            let local = std::array::from_fn(|n| {
                let x = (bx + nx + n % 3 - 1) % nx;
                let y = (by + ny + n / 3 - 1) % ny;
                texel(data, morton(x, y, nx.min(ny)), is2)
            });
            for y in 0..4.min(h - by * 4) {
                for x in 0..bw.min(w - bx * bw) {
                    let offset = ((h - 1 - by * 4 - y) * w + bx * bw + x) * 4;
                    if let Some(dst) = out.get_mut(offset..offset + 4) {
                        dst.copy_from_slice(&pixel(&local, x, y, bw));
                    }
                }
            }
        }
    }
}
