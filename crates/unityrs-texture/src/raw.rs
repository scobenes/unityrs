// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/RawDecode.c's row loop, and src/libImaging/Unpack.c's unpackers for the raw modes UnityPy names (copy for RGB and RGBA, band3 for A, band0 for R, ImagingUnpackBGR, unpackBGRA, unpackARGB, ImagingUnpackBGR16 and ImagingUnpackRGBA4B), each on an image Image.new zeroed. See THIRD_PARTY_LICENSES.
// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's pillow (Image.frombytes, then the band reorder of its swap) and the raw modes and swaps of CONV_TABLE. See THIRD_PARTY_LICENSES.
//! Pillow 9.5.0's raw decoder, as UnityPy 1.25.2 calls it for the raw
//! formats (`pillow`, `export/Texture2DConverter.py:331-344`, with
//! `CONV_TABLE`'s mode, raw mode and band order): the data is read row by
//! row, each pixel through the raw mode's unpacker into a pixel of the
//! image's mode (`RawDecode.c`'s `ImagingRawDecode`), on an image
//! `Image.frombytes` made with `Image.new`, so zeroed: a band the unpacker
//! does not write stays 0. Each row is a whole number of bytes here. Then,
//! for the two formats `CONV_TABLE` gives a band order, UnityPy splits the
//! image's bands and merges them in that order (`:341-343`), and flips the
//! image top to bottom. The unpackers, each pixel `[r, g, b, a]` as stored
//! in Pillow's four-byte pixel, the image's mode keeping its first three
//! bands for `RGB`:
//! - `RGB` and `RGBA` (RGB24, RGBA32; `:491-492`): the bytes as stored
//!   (Pillow's `RGB` unpacker adds an alpha of 255, which `RGB` drops).
//! - `A` (Alpha8, into `RGBA`; `:489`): `band3`, the byte into alpha: (0,
//!   0, 0, a).
//! - `R` (R8, into `RGB`; `:497`): `band0`, the byte into red: (r, 0, 0).
//! - `BGR` (BGR24, into `RGB`; `:496`): `ImagingUnpackBGR`, the bytes
//!   reversed.
//! - `BGRA` (BGRA32; `:504`): `unpackBGRA`, (in\[2\], in\[1\], in\[0\],
//!   in\[3\]).
//! - `ARGB` (ARGB32; `:493`): `unpackARGB`, (in\[1\], in\[2\], in\[3\],
//!   in\[0\]).
//! - `BGR;16` (RGB565, into `RGB`; `:495`): `ImagingUnpackBGR16`, a
//!   little-endian 16-bit pixel whose low 5 bits are blue, next 6 green,
//!   top 5 red, each widened as `v * 255 / 31` (green `/ 63`), truncated.
//! - `RGBA;4B` (ARGB4444 and RGBA4444; `:490`, `:503`):
//!   `ImagingUnpackRGBA4B`, a little-endian 16-bit pixel of four 4-bit
//!   fields, red lowest, each widened as `v * 17`; then the bands
//!   reordered: band `i` of the result is band `swap[i]` of the unpacked
//!   image, (2, 1, 0, 3) for ARGB4444, (3, 2, 1, 0) for RGBA4444.

/// Which unpacker, and which band order after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `RGB` into `RGB`: RGB24.
    Rgb,
    /// `RGBA` into `RGBA`: RGBA32.
    Rgba,
    /// `A` into `RGBA`: Alpha8.
    A,
    /// `R` into `RGB`: R8.
    R,
    /// `BGR` into `RGB`: BGR24.
    Bgr,
    /// `BGRA` into `RGBA`: BGRA32.
    Bgra,
    /// `ARGB` into `RGBA`: ARGB32.
    Argb,
    /// `BGR;16` into `RGB`: RGB565.
    Bgr16,
    /// `RGBA;4B` into `RGBA`, bands (2, 1, 0, 3): ARGB4444.
    Rgba4BArgb,
    /// `RGBA;4B` into `RGBA`, bands (3, 2, 1, 0): RGBA4444.
    Rgba4BRgba,
}

/// A value Pillow computes in an `int` and stores in a `UINT8`, never past
/// 255 here.
fn byte(v: u32) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

/// `ImagingUnpackBGR16`.
fn bgr16(p: [u8; 2]) -> [u8; 3] {
    let pixel = u32::from(u16::from_le_bytes(p));
    let b = (pixel & 31) * 255 / 31;
    let g = ((pixel >> 5) & 63) * 255 / 63;
    let r = ((pixel >> 11) & 31) * 255 / 31;
    [byte(r), byte(g), byte(b)]
}

/// `ImagingUnpackRGBA4B`: `[r, g, b, a]`.
fn rgba4b(p: [u8; 2]) -> [u8; 4] {
    let pixel = u32::from(u16::from_le_bytes(p));
    [0, 4, 8, 12].map(|shift| byte(((pixel >> shift) & 15) * 17))
}

/// Decodes `data` (at least `h` rows of `w` stored pixels) into `out`, the
/// `w` x `h` image's rows top to bottom: stored row `y` is output row
/// `h - 1 - y`, UnityPy's flip.
// The bands' names, `r`, `g`, `b` and `a`, beside the sides' `w` and `h`.
#[allow(clippy::many_single_char_names)]
pub(crate) fn decode(kind: Kind, data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    match kind {
        Kind::Rgb => copy(data, w.saturating_mul(3), h, out),
        Kind::Rgba => copy(data, w.saturating_mul(4), h, out),
        Kind::A => rows(data, w, h, out, |&[a]: &[u8; 1]| [0, 0, 0, a]),
        Kind::R => rows(data, w, h, out, |&[r]: &[u8; 1]| [r, 0, 0]),
        Kind::Bgr => rows(data, w, h, out, |&[b, g, r]: &[u8; 3]| [r, g, b]),
        Kind::Bgra => rows(data, w, h, out, |&[b, g, r, a]: &[u8; 4]| [r, g, b, a]),
        Kind::Argb => rows(data, w, h, out, |&[a, r, g, b]: &[u8; 4]| [r, g, b, a]),
        Kind::Bgr16 => rows(data, w, h, out, |p: &[u8; 2]| bgr16(*p)),
        // `swap` (2, 1, 0, 3): (b, g, r, a).
        Kind::Rgba4BArgb => rows(data, w, h, out, |p: &[u8; 2]| {
            let [r, g, b, a] = rgba4b(*p);
            [b, g, r, a]
        }),
        // `swap` (3, 2, 1, 0): (a, b, g, r).
        Kind::Rgba4BRgba => rows(data, w, h, out, |p: &[u8; 2]| {
            let [r, g, b, a] = rgba4b(*p);
            [a, b, g, r]
        }),
    }
}

/// A raw mode that is the image's own: whole rows copied, flipped.
fn copy(data: &[u8], row: usize, h: usize, out: &mut [u8]) {
    if row == 0 {
        return;
    }
    for (r, dst) in out.chunks_exact_mut(row).enumerate() {
        let from = h.saturating_sub(1).saturating_sub(r).saturating_mul(row);
        if let Some(src) = data.get(from..from.saturating_add(row)) {
            dst.copy_from_slice(src);
        }
    }
}

/// Every row through `unpack`, `IN` stored bytes a pixel to `OUT` output
/// bytes, flipped.
fn rows<const IN: usize, const OUT: usize>(
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
    unpack: impl Fn(&[u8; IN]) -> [u8; OUT],
) {
    let (src_row, dst_row) = (w.saturating_mul(IN), w.saturating_mul(OUT));
    if dst_row == 0 {
        return;
    }
    for (r, dst) in out.chunks_exact_mut(dst_row).enumerate() {
        let from = h
            .saturating_sub(1)
            .saturating_sub(r)
            .saturating_mul(src_row);
        let Some(src) = data.get(from..from.saturating_add(src_row)) else {
            continue;
        };
        let pixels = dst.as_chunks_mut::<OUT>().0.iter_mut();
        for (d, s) in pixels.zip(src.as_chunks::<IN>().0) {
            *d = unpack(s);
        }
    }
}

#[cfg(test)]
mod tests;
