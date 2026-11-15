// Ported from texture2ddecoder 1.0.5 (MIT): src/Texture2DDecoder/atc.cpp's decode_atc_block, decode_atc_rgb4 and decode_atc_rgba8, bcn.cpp's decode_bc3_alpha, and color.h's copy_block_buffer. See THIRD_PARTY_LICENSES.
//! ATC RGB and interpolated-alpha RGBA. Palette arithmetic retains the
//! reference's byte conversions and its clamped subtractive entry (G20).
//! Blocks are row-major; UnityPy leaves ATC sizes unpadded. Store only the
//! visible pixels, flipped vertically, directly in the metered output.

/// Decode mip 0 after `plan` checked its block bytes.
pub(crate) fn decode(data: &[u8], w: usize, h: usize, alpha: bool, out: &mut [u8]) {
    let size = if alpha { 16 } else { 8 };
    let nx = w.div_ceil(4);
    for (n, src) in data.chunks_exact(size).take(nx * h.div_ceil(4)).enumerate() {
        let colour = if alpha {
            src.get(8..).unwrap_or_default()
        } else {
            src
        };
        let mut pixels = rgb(colour);
        if alpha {
            let a0 = src.first().copied().unwrap_or(0);
            let a1 = src.get(1).copied().unwrap_or(0);
            let mut palette = [a0, a1, 0, 0, 0, 0, 0, 255];
            let (a0, a1) = (u32::from(a0), u32::from(a1));
            let divisor = if a0 > a1 { 7 } else { 5 };
            for (i, a) in palette
                .iter_mut()
                .enumerate()
                .take(usize::try_from(divisor).unwrap_or(0) + 1)
                .skip(2)
            {
                let k = u32::try_from(i).unwrap_or(1) - 1;
                *a = u8::try_from(((divisor - k) * a0 + k * a1) / divisor).unwrap_or(0);
            }
            let bits = src
                .iter()
                .take(8)
                .enumerate()
                .fold(0u64, |v, (i, b)| v | (u64::from(*b) << (8 * i)))
                >> 16;
            for (i, [_, _, _, a]) in pixels.iter_mut().enumerate() {
                *a = palette
                    .get(((bits >> (3 * i)) & 7) as usize)
                    .copied()
                    .unwrap_or(0);
            }
        }
        for (i, pixel) in pixels.iter().enumerate() {
            let (x, y) = (n % nx * 4 + i % 4, n / nx * 4 + i / 4);
            if x < w && y < h {
                let offset = ((h - 1 - y) * w + x) * 4;
                if let Some(dst) = out.get_mut(offset..offset + 4) {
                    dst.copy_from_slice(pixel);
                }
            }
        }
    }
}

fn rgb(src: &[u8]) -> [[u8; 4]; 16] {
    let word = |i| {
        u16::from_le_bytes([
            src.get(i).copied().unwrap_or(0),
            src.get(i + 1).copied().unwrap_or(0),
        ])
    };
    let (c0, c1) = (word(0), word(2));
    let expand = |v: u16, bits: u32| {
        let v = u8::try_from(v << (8 - bits)).unwrap_or(0);
        v | (v >> bits)
    };
    let a = [
        expand((c0 >> 10) & 31, 5),
        expand((c0 >> 5) & 31, 5),
        expand(c0 & 31, 5),
    ];
    let b = [
        expand((c1 >> 11) & 31, 5),
        expand((c1 >> 5) & 63, 6),
        expand(c1 & 31, 5),
    ];
    let mut palette = [[0, 0, 0, 255]; 4];
    for (ch, (a, b)) in a.into_iter().zip(b).enumerate() {
        let (a, b) = (u16::from(a), u16::from(b));
        let values = if c0 & 0x8000 == 0 {
            [a, (5 * a + 3 * b) / 8, (3 * a + 5 * b) / 8, b]
        } else {
            [0, a.saturating_sub(b / 4), a, b]
        };
        for (colour, v) in palette.iter_mut().zip(values) {
            if let Some(c) = colour.get_mut(ch) {
                *c = u8::try_from(v).unwrap_or(0);
            }
        }
    }
    std::array::from_fn(|i| {
        let index = (src.get(4 + i / 4).copied().unwrap_or(0) >> (2 * (i % 4))) & 3;
        palette.get(usize::from(index)).copied().unwrap_or([0; 4])
    })
}
