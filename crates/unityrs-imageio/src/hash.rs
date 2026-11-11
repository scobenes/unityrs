// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/Convert.c's l2rgb, la2rgb and rgb2rgba, the converters of convert("RGBA") from L, LA and RGB. See THIRD_PARTY_LICENSES.
//! The visible-pixel hash (`docs/plans/extract.md` E20): SHA-256 over the
//! ASCII bytes `"{width}x{height}:"`, then the image's RGBA bytes in row
//! order, each pixel of alpha 0 written as (0, 0, 0, 0). An image in another
//! mode is converted to RGBA first, as Pillow 9.5.0's `convert("RGBA")`
//! converts it (its converters' table: Pillow 9.5.0,
//! `src/libImaging/Convert.c:902`, `Convert.c:911`, `Convert.c:937`): `L`
//! to (l, l, l, 255) (`l2rgb`, `Convert.c:161-170`), `LA` to (l, l, l, a)
//! (`la2rgb`, `Convert.c:193-202`) and `RGB` to (r, g, b, 255)
//! (`rgb2rgba`, `Convert.c:415-424`). Only `LA` and `RGBA` can hold alpha 0.
//!
//! The pixels are converted and hashed in a buffer on the stack, so the
//! hash allocates nothing.

use sha2::{Digest, Sha256};

use crate::image::{Image, Mode};

/// RGBA pixels converted and hashed at a time.
const CHUNK_PIXELS: usize = 1024;

/// The visible-pixel hash of `image` (module docs).
#[must_use]
pub fn visible_hash(image: &Image<'_>) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(format!("{}x{}:", image.width(), image.height()).as_bytes());
    let channels = image.mode().channels();
    let mut rgba = [0_u8; CHUNK_PIXELS * 4];
    for pixels in image.pixels().chunks(CHUNK_PIXELS * channels) {
        let mut n = 0;
        let (out, _) = rgba.as_chunks_mut::<4>();
        for (out, px) in out.iter_mut().zip(pixels.chunks_exact(channels)) {
            *out = visible(image.mode(), px);
            n += 4;
        }
        sha.update(rgba.get(..n).unwrap_or_default());
    }
    sha.finalize().into()
}

/// One pixel of `mode` as RGBA, cleared to (0, 0, 0, 0) when its alpha is 0.
#[inline]
fn visible(mode: Mode, px: &[u8]) -> [u8; 4] {
    let rgba = match (mode, px) {
        (Mode::L, &[l]) => [l, l, l, 255],
        (Mode::La, &[l, a]) => [l, l, l, a],
        (Mode::Rgb, &[r, g, b]) => [r, g, b, 255],
        (Mode::Rgba, &[r, g, b, a]) => [r, g, b, a],
        // `chunks_exact(channels)` gives no other length.
        _ => [0; 4],
    };
    let [_, _, _, alpha] = rgba;
    if alpha == 0 { [0; 4] } else { rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cleared_pixel_hashes_as_zeros_and_the_size_is_hashed() {
        let a = Image::new(1, 1, Mode::Rgba, &[9, 8, 7, 0]).unwrap();
        let b = Image::new(1, 1, Mode::Rgba, &[0, 0, 0, 0]).unwrap();
        assert_eq!(visible_hash(&a), visible_hash(&b));
        // sha256(b"1x1:" + b"\0\0\0\0")
        assert_eq!(
            visible_hash(&a),
            <[u8; 32]>::from(Sha256::digest(b"1x1:\0\0\0\0"))
        );
        let c = Image::new(1, 1, Mode::La, &[5, 0]).unwrap();
        assert_eq!(visible_hash(&c), visible_hash(&b));
        let tall = Image::new(1, 4, Mode::L, &[0; 4]).unwrap();
        let wide = Image::new(4, 1, Mode::L, &[0; 4]).unwrap();
        assert_ne!(visible_hash(&tall), visible_hash(&wide));
    }

    #[test]
    fn other_modes_hash_as_pillows_rgba() {
        let l = Image::new(2, 1, Mode::L, &[3, 250]).unwrap();
        let rgba = Image::new(2, 1, Mode::Rgba, &[3, 3, 3, 255, 250, 250, 250, 255]).unwrap();
        assert_eq!(visible_hash(&l), visible_hash(&rgba));
        let rgb = Image::new(1, 1, Mode::Rgb, &[1, 2, 3]).unwrap();
        let rgba = Image::new(1, 1, Mode::Rgba, &[1, 2, 3, 255]).unwrap();
        assert_eq!(visible_hash(&rgb), visible_hash(&rgba));
        // An image of more than one chunk.
        let px: Vec<u8> = (0..3000_u32).map(|i| (i % 251) as u8).collect();
        let la = Image::new(1500, 1, Mode::La, &px).unwrap();
        let mut expect = Sha256::new();
        expect.update(b"1500x1:");
        for p in px.chunks(2) {
            let v = if p[1] == 0 {
                [0; 4]
            } else {
                [p[0], p[0], p[0], p[1]]
            };
            expect.update(v);
        }
        assert_eq!(visible_hash(&la), <[u8; 32]>::from(expect.finalize()));
    }
}
