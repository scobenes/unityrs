// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image (the alpha merge) and the crop and transposes of get_image_from_sprite. See THIRD_PARTY_LICENSES.
// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/Image.py's crop, _crop and _decompression_bomb_check, src/libImaging/Crop.c's ImagingCrop, src/libImaging/Geometry.c's ImagingFlipLeftRight, ImagingFlipTopBottom, ImagingRotate180 and ImagingRotate270, Image.merge's size check, and src/libImaging/Storage.c's ImagingNewPrologueSubtype (its width cap). See THIRD_PARTY_LICENSES.
//! The images a render works in: the [`Sheet`] it reads, and the canvases
//! it writes (a crop, a transpose of it, a mask's result, a mesh's
//! canvas), each charged to the caller's meter before it is allocated.
//!
//! **The sheet's orientation.** `get_image` decodes the texture unflipped
//! (`export/SpriteHelper.py:47-58`), and [`crate::decode()`] flips it, so
//! row `y` of the sheet UnityPy reads is row `height - 1 - y` of the
//! decoded image ([`Sheet::pixel`]). A merged sheet is built in the
//! decoded orientation, row for row, so the rule holds for it too.
//!
//! **Pillow's crop** (`Image.crop`, `src/PIL/Image.py:1206-1250`):
//! the box's right edge is compared with its left, then its lower with its
//! upper, as Python `float`s (a NaN compares false, so it passes); each
//! edge is then rounded half to even (`round`: a NaN raises `ValueError`,
//! an infinity `OverflowError`); the crop's size, `abs(x1 - x0) x abs(y1 -
//! y0)` in Python's integers, is checked against twice
//! `MAX_IMAGE_PIXELS` (`DecompressionBombError`, naming that product
//! exactly); and `im.crop` takes the four as C `int`s (`OverflowError`
//! past one, worded as `PyLong_AsLong`'s past a C `long`), in that order.
//! The C crop (`ImagingCrop`) sizes the image by `x1 - x0` and `y1 - y0` in
//! `int`, a negative side made 0, and fills what lies outside the image
//! with zeros.
//!
//! **Pillow's width cap.** Every image Pillow makes passes
//! `ImagingNewPrologueSubtype`, which raises `MemoryError` for a width past
//! `INT_MAX / 4 - 1` whatever the height (`src/libImaging/Storage.c`), so
//! a 536,870,911 x 0 crop is refused where no pixel is held:
//! [`width_cap`], which [`Canvas::new`] checks first, and the render
//! wherever UnityPy makes an image it does not.

// The C and Python names they port: `x0`, `a`, `b`, `xx`.
// Floats compared exactly, as the ported code compares them.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::float_cmp
)]

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, MeteredVec};

use super::SpriteError;
use super::exact::Int;
use crate::MAX_PIXELS;
use crate::decode::{Image, MAX_CROP_PIXELS};
use crate::format::Mode;

/// What a sprite renders from (module docs): the decoded texture, or the
/// texture with its alpha texture merged in.
#[derive(Debug)]
pub struct Sheet<'a> {
    image: SheetImage<'a>,
}

#[derive(Debug)]
enum SheetImage<'a> {
    Texture(&'a Image),
    Merged(Image),
}

const _: () = crate::send_sync::<Sheet<'static>>();

impl<'a> Sheet<'a> {
    /// `get_image` (`export/SpriteHelper.py:44-59`): `texture` as
    /// [`crate::decode()`] gave it, and, where the render data names an
    /// alpha texture, the first three bands of `texture` with the first of
    /// `alpha` (`Image.merge("RGBA", ...)`, `:50-52`), an `RGBA` image
    /// charged to `meter` before it is allocated.
    ///
    /// The renderer is verified on `RGB` and `RGBA` sheets, the five
    /// formats' (`PARITY.md` section 19.2), so it takes no other, and
    /// refuses before anything is charged or drawn:
    /// - a texture that decodes to `L` (BC4) beside an alpha texture is
    ///   [`SpriteError::AlphaBandCount`], `Image.merge`'s `ValueError`
    ///   "wrong number of bands", which it raises before it compares sizes;
    /// - decoded sizes that differ are [`SpriteError::AlphaSizeMismatch`],
    ///   its "size mismatch";
    /// - an `L` texture alone, or an `L` alpha texture of the texture's
    ///   size, which UnityPy renders, is [`SpriteError::UnverifiedMode`],
    ///   unityrs's own refusal (Tier U) until a render of them is checked
    ///   against UnityPy's.
    ///
    /// UnityPy's cache of the merged sheet is the caller's: a sheet serves
    /// every sprite of its group.
    pub fn new(
        texture: &'a Image,
        alpha: Option<&Image>,
        meter: &Arc<Meter>,
    ) -> Result<Self, SpriteError> {
        match alpha {
            None => {
                verified(texture.mode())?;
                Ok(Self {
                    image: SheetImage::Texture(texture),
                })
            }
            Some(alpha) => Sheet::merged(texture, alpha, meter),
        }
    }

    /// [`Sheet::new`] with an alpha texture: the merged sheet, which
    /// borrows neither image, so the caller may drop both once it is made.
    pub fn merged(
        texture: &Image,
        alpha: &Image,
        meter: &Arc<Meter>,
    ) -> Result<Sheet<'static>, SpriteError> {
        // `Image.merge` counts the bands before it compares sizes; one band
        // and the alpha's are two, where `RGBA` needs four.
        if texture.mode() == Mode::L {
            return Err(SpriteError::AlphaBandCount);
        }
        if texture.width() != alpha.width() || texture.height() != alpha.height() {
            return Err(SpriteError::AlphaSizeMismatch);
        }
        verified(alpha.mode())?;
        let mut merged = Canvas::new(Mode::Rgba, texture.width(), texture.height(), meter)?;
        let tb = bpp(texture.mode());
        let ab = bpp(alpha.mode());
        let (tp, ap) = (texture.pixels(), alpha.pixels());
        super::contained(|| {
            for (i, out) in merged.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let t = tp.get(i * tb..i * tb + 3).unwrap_or(&[0; 3]);
                out.get_mut(..3).unwrap_or_default().copy_from_slice(t);
                if let (Some(o), Some(a)) = (out.get_mut(3), ap.get(i * ab)) {
                    *o = *a;
                }
            }
        })?;
        Ok(Sheet {
            image: SheetImage::Merged(merged.into_image()),
        })
    }

    fn image(&self) -> &Image {
        match &self.image {
            SheetImage::Texture(i) => i,
            SheetImage::Merged(i) => i,
        }
    }

    /// The mode: the texture's, or `RGBA` when merged.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.image().mode()
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.image().width()
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.image().height()
    }

    /// The pixel at `(x, y)` of the sheet as UnityPy reads it, unflipped
    /// (module docs), `None` outside it.
    pub(crate) fn pixel(&self, x: i64, y: i64) -> Option<&[u8]> {
        let image = self.image();
        let (w, h) = (i64::from(image.width()), i64::from(image.height()));
        if x < 0 || y < 0 || x >= w || y >= h {
            return None;
        }
        let b = bpp(image.mode());
        let at =
            usize::try_from(((h - 1 - y) * w + x).checked_mul(i64::try_from(b).ok()?)?).ok()?;
        image.pixels().get(at..at.checked_add(b)?)
    }
}

/// A sheet's or an alpha texture's mode the renderer is verified on:
/// `RGB` or `RGBA` ([`Sheet::new`]).
fn verified(mode: Mode) -> Result<(), SpriteError> {
    match mode {
        Mode::Rgb | Mode::Rgba => Ok(()),
        Mode::L => Err(SpriteError::UnverifiedMode { mode: mode.name() }),
    }
}

/// The bytes of one pixel of `mode`.
pub(crate) const fn bpp(mode: Mode) -> usize {
    match mode {
        Mode::L => 1,
        Mode::Rgb => 3,
        Mode::Rgba => 4,
    }
}

/// An image being written: top row first, as Pillow holds it, zeroed.
#[derive(Debug)]
pub(crate) struct Canvas {
    pub(crate) mode: Mode,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
    // After `pixels`: the charge outlives the buffer it pays for.
    charge: Charge,
}

/// The widest image Pillow makes, `INT_MAX / 4 - 1` (module docs).
pub(crate) const WIDTH_CAP: u32 = 536_870_910;

/// Pillow's width cap on an image `width` wide (module docs):
/// [`SpriteError::ImageTooWide`] past [`WIDTH_CAP`].
pub(crate) fn width_cap(width: u32) -> Result<(), SpriteError> {
    if width > WIDTH_CAP {
        Err(SpriteError::ImageTooWide)
    } else {
        Ok(())
    }
}

impl Canvas {
    /// A zeroed canvas, `Image.new(mode, (width, height))`, charged to
    /// `meter` before it is allocated: Pillow's width cap first
    /// ([`width_cap`]), then, past [`MAX_PIXELS`] pixels,
    /// [`SpriteError::TooLarge`]. Its work is its bytes and its rows, one
    /// each: Pillow callocs a pointer for every row of every image, an
    /// empty one's included (`ImagingNewPrologueSubtype`,
    /// `src/libImaging/Storage.c:205`), though `ImagingAllocateArray`
    /// returns before its row loop for one 0 wide (`Storage.c:385-388`),
    /// and the transposes step through every row of one 0 wide
    /// (`src/libImaging/Geometry.c:32-39`, `Geometry.c:76-79`), as
    /// UnityPy's final flip does on every sprite. So a canvas 0 wide and
    /// many rows high is work a zero allowance refuses.
    pub(crate) fn new(
        mode: Mode,
        width: u32,
        height: u32,
        meter: &Arc<Meter>,
    ) -> Result<Self, SpriteError> {
        width_cap(width)?;
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_PIXELS {
            return Err(SpriteError::TooLarge { what: "canvas" });
        }
        let len = usize::try_from(pixels * bpp(mode) as u64)
            .map_err(|_| SpriteError::TooLarge { what: "canvas" })?;
        meter.count_decode(len as u64 + u64::from(height))?;
        let (mut buf, charge) = MeteredVec::<u8>::with_capacity(meter, len)?.into_parts();
        // Within the capacity just reserved: no allocation.
        buf.resize(len, 0);
        Ok(Self {
            mode,
            width,
            height,
            pixels: buf,
            charge,
        })
    }

    fn at(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * bpp(self.mode)
    }

    /// The pixel at `(x, y)`, which the caller keeps inside.
    pub(crate) fn get(&self, x: u32, y: u32) -> &[u8] {
        let at = self.at(x, y);
        self.pixels.get(at..at + bpp(self.mode)).unwrap_or_default()
    }

    /// The pixel at `(x, y)`, to write.
    pub(crate) fn get_mut(&mut self, x: u32, y: u32) -> &mut [u8] {
        let at = self.at(x, y);
        let b = bpp(self.mode);
        self.pixels.get_mut(at..at + b).unwrap_or_default()
    }

    /// The image, as it stands.
    pub(crate) fn into_image(self) -> Image {
        Image::from_parts(self.mode, self.width, self.height, self.pixels, self.charge)
    }

    /// The work of a pass over the canvas, its bytes and its rows, counted
    /// before it runs, an empty canvas's rows included.
    fn count_pass(&self, meter: &Meter) -> Result<(), SpriteError> {
        meter.count_decode(self.pixels.len() as u64 + u64::from(self.height))?;
        Ok(())
    }

    /// `transpose(FLIP_TOP_BOTTOM)`, in place.
    pub(crate) fn flip_top_bottom(&mut self, meter: &Meter) -> Result<(), SpriteError> {
        self.count_pass(meter)?;
        let row = self.width as usize * bpp(self.mode);
        let h = self.height as usize;
        let pixels = &mut self.pixels;
        // An empty canvas has nothing to move; its rows are counted above.
        if row == 0 {
            return Ok(());
        }
        super::contained(|| {
            for y in 0..h / 2 {
                let (top, bottom) = pixels.split_at_mut((h - 1 - y) * row);
                if let (Some(a), Some(b)) =
                    (top.get_mut(y * row..(y + 1) * row), bottom.get_mut(..row))
                {
                    a.swap_with_slice(b);
                }
            }
        })
    }

    /// `transpose(FLIP_LEFT_RIGHT)`, in place.
    pub(crate) fn flip_left_right(&mut self, meter: &Meter) -> Result<(), SpriteError> {
        self.count_pass(meter)?;
        let b = bpp(self.mode);
        let row = self.width as usize * b;
        let pixels = &mut self.pixels;
        super::contained(|| {
            if row == 0 {
                return;
            }
            for line in pixels.chunks_exact_mut(row) {
                let w = line.len() / b;
                for x in 0..w / 2 {
                    let (l, r) = line.split_at_mut((w - 1 - x) * b);
                    if let (Some(p), Some(q)) = (l.get_mut(x * b..(x + 1) * b), r.get_mut(..b)) {
                        p.swap_with_slice(q);
                    }
                }
            }
        })
    }

    /// `transpose(ROTATE_180)`, in place: both flips.
    pub(crate) fn rotate_180(&mut self, meter: &Meter) -> Result<(), SpriteError> {
        self.flip_top_bottom(meter)?;
        self.flip_left_right(meter)
    }

    /// `transpose(ROTATE_270)`, a quarter turn clockwise, into a new canvas
    /// of the transposed size: output row `x` holds input column `x`, read
    /// from the bottom (`ImagingRotate270`: `out[x][ysize - 1 - y] =
    /// in[y][x]`).
    pub(crate) fn rotate_270(&self, meter: &Arc<Meter>) -> Result<Self, SpriteError> {
        let mut out = Self::new(self.mode, self.height, self.width, meter)?;
        let (w, h) = (self.width, self.height);
        // The input's rows, read; the output's were counted as it was made.
        meter.count_decode(u64::from(h))?;
        if w == 0 || h == 0 {
            return Ok(out);
        }
        super::contained(|| {
            for y in 0..h {
                for x in 0..w {
                    let p = self.get(x, y);
                    out.get_mut(h - 1 - y, x).copy_from_slice(p);
                }
            }
        })?;
        Ok(out)
    }
}

/// Python's `round(v)` of a `float`: half to even, an integral `f64`; a NaN
/// is `ValueError`, an infinity `OverflowError`.
pub(crate) fn py_round(v: f64) -> Result<f64, SpriteError> {
    if v.is_nan() {
        Err(SpriteError::RoundNan)
    } else if v.is_infinite() {
        Err(SpriteError::RoundInfinity)
    } else {
        Ok(v.round_ties_even())
    }
}

/// An integral `f64` as `PyArg_ParseTuple`'s `i` takes it, a C `int`.
pub(crate) fn c_int(v: f64) -> Result<i32, SpriteError> {
    c_int_of(Int::of_f64(v))
}

/// A Python `int` as `PyArg_ParseTuple`'s `i` takes it: `PyLong_AsLong`
/// first (past a C `long`, [`SpriteError::CLongOverflow`]), then the
/// range of an `int` ([`SpriteError::CIntOverflow`]).
pub(crate) fn c_int_of(v: Int) -> Result<i32, SpriteError> {
    let Int::I(i) = v else {
        return Err(SpriteError::CLongOverflow);
    };
    if i64::try_from(i).is_err() {
        return Err(SpriteError::CLongOverflow);
    }
    i32::try_from(i).map_err(|_| SpriteError::CIntOverflow { above: i > 0 })
}

/// C's `(int)v` of a `double` as x86-64's `cvttsd2si` gives it: toward
/// zero, and `INT_MIN` for a NaN or a value past an `int`, where C leaves
/// the conversion undefined (Pillow's wheel is built for x86-64 with GCC).
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn c_trunc(v: f64) -> i32 {
    if v > -2_147_483_649.0 && v < 2_147_483_648.0 {
        v as i32
    } else {
        i32::MIN
    }
}

/// `Image.crop(box)` of `textureRect` up to the C crop (module docs): the
/// box as C `int`s, or UnityPy's error, in its order.
pub(crate) fn crop_box(rect: [f64; 4]) -> Result<[i32; 4], SpriteError> {
    let [x, y, w, h] = rect;
    let edges = [x, y, x + w, y + h];
    let [x0, y0, x1, y1] = edges;
    if x1 < x0 {
        return Err(SpriteError::CropRightLeft);
    }
    if y1 < y0 {
        return Err(SpriteError::CropLowerUpper);
    }
    let mut r = [Int::I(0); 4];
    for (out, e) in r.iter_mut().zip(edges) {
        *out = Int::of_f64(py_round(e)?);
    }
    bomb_check(r)?;
    let mut out = [0; 4];
    for (o, e) in out.iter_mut().zip(r) {
        *o = c_int_of(e)?;
    }
    Ok(out)
}

/// `_decompression_bomb_check((abs(x1 - x0), abs(y1 - y0)))` of a box of
/// Python `int`s (`src/PIL/Image.py:3162-3180`), its message naming the
/// product exactly; past `MAX_IMAGE_PIXELS` alone it only warns.
pub(crate) fn bomb_check([x0, y0, x1, y1]: [Int; 4]) -> Result<(), SpriteError> {
    let pixels = x1.abs_diff(x0).mul(&y1.abs_diff(y0));
    if pixels.above(MAX_CROP_PIXELS) {
        return Err(SpriteError::DecompressionBomb {
            pixels: pixels.decimal(),
        });
    }
    Ok(())
}

/// The size `ImagingCrop` gives a box of C `int`s: its sides in `int`
/// (GCC's wrapping subtraction), a negative one 0.
pub(crate) fn crop_size([x0, y0, x1, y1]: [i32; 4]) -> (u32, u32) {
    let side = |a: i32, b: i32| u32::try_from(b.wrapping_sub(a).max(0)).unwrap_or(0);
    (side(x0, x1), side(y0, y1))
}

/// `ImagingCrop` of `sheet` by `box` (module docs): its sides as
/// [`crop_size`] gives them; what lies outside the sheet zero.
pub(crate) fn crop(
    sheet: &Sheet<'_>,
    crop_box: [i32; 4],
    meter: &Arc<Meter>,
) -> Result<Canvas, SpriteError> {
    let [x0, y0, ..] = crop_box;
    let (w, h) = crop_size(crop_box);
    let mut out = Canvas::new(sheet.mode(), w, h, meter)?;
    // The rows copied, an empty crop's included (its bytes were counted as
    // it was made).
    meter.count_decode(u64::from(h))?;
    if w == 0 || h == 0 {
        return Ok(out);
    }
    super::contained(|| {
        for y in 0..h {
            for x in 0..w {
                if let Some(p) =
                    sheet.pixel(i64::from(x0) + i64::from(x), i64::from(y0) + i64::from(y))
                {
                    out.get_mut(x, y).copy_from_slice(p);
                }
            }
        }
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, decode};

    /// A BC4 texture decodes to `L`, which the renderer is not verified on
    /// (`Sheet::new`): beside an alpha texture it is `Image.merge`'s "wrong
    /// number of bands", before the sizes are compared; alone, or as an
    /// alpha texture of the texture's size, unityrs's own refusal; a size
    /// mismatch is still `Image.merge`'s. Nothing is charged, and no
    /// sheet is made.
    #[test]
    fn an_l_sheet_or_alpha_is_refused_before_anything_is_drawn() {
        let meter = Meter::unlimited();
        let l = decode(Format::Bc4, &[0; 8], 4, 4, &meter).unwrap();
        let wide_l = decode(Format::Bc4, &[0; 16], 8, 4, &meter).unwrap();
        let rgba = decode(Format::Rgba32, &[0; 64], 4, 4, &meter).unwrap();
        let wide = decode(Format::Rgba32, &[0; 128], 8, 4, &meter).unwrap();
        let rgb = decode(Format::Rgb24, &[0; 48], 4, 4, &meter).unwrap();
        assert_eq!(l.mode(), Mode::L);
        let held = meter.used();
        let refused = |texture: &Image, alpha: Option<&Image>| {
            Sheet::new(texture, alpha, &meter)
                .map(|_| ())
                .expect_err("refused")
        };
        // The alpha path: UnityPy's errors, the band count first.
        for alpha in [&rgba, &rgb, &l, &wide] {
            let e = refused(&l, Some(alpha));
            assert!(matches!(e, SpriteError::AlphaBandCount), "{e:?}");
            assert_eq!(
                e.python(),
                Some(("ValueError", "wrong number of bands".to_owned()))
            );
        }
        let e = refused(&rgba, Some(&wide_l));
        assert!(matches!(e, SpriteError::AlphaSizeMismatch), "{e:?}");
        // An `L` alpha texture of the texture's size, and an `L` texture
        // alone: unityrs's refusal, no exception of UnityPy's.
        for e in [
            refused(&rgba, Some(&l)),
            refused(&rgb, Some(&l)),
            refused(&l, None),
        ] {
            assert!(
                matches!(e, SpriteError::UnverifiedMode { mode: "L" }),
                "{e:?}"
            );
            assert_eq!(e.python(), None);
        }
        assert_eq!(meter.used(), held);
        // The verified modes still make sheets.
        assert!(Sheet::new(&rgb, None, &meter).is_ok());
        assert!(Sheet::new(&rgb, Some(&rgba), &meter).is_ok());
    }

    /// The renderer's bounds (`PARITY.md` section 19.3): Pillow's width cap
    /// first, whatever the height, then the pixel bound, before anything
    /// is charged.
    #[test]
    fn a_canvas_past_the_bounds_is_refused_before_it_is_charged() {
        let meter = Meter::unlimited();
        let wide = Canvas::new(Mode::Rgba, WIDTH_CAP + 1, 0, &meter);
        assert!(matches!(wide, Err(SpriteError::ImageTooWide)));
        assert_eq!(
            wide.err().and_then(|e| e.python()),
            Some(("MemoryError", String::new()))
        );
        assert!(Canvas::new(Mode::Rgba, WIDTH_CAP, 0, &meter).is_ok());
        let big = Canvas::new(Mode::Rgb, 16_385, 16_384, &meter);
        assert!(matches!(big, Err(SpriteError::TooLarge { what: "canvas" })));
        assert_eq!(meter.used(), 0);
    }

    #[test]
    fn a_box_edge_is_taken_as_pyarg_s_i_takes_it() {
        assert_eq!(c_int(-3.0).ok(), Some(-3));
        let err = |v: f64| c_int(v).err().and_then(|e| e.python());
        let over = |m: &str| Some(("OverflowError", m.to_owned()));
        assert_eq!(
            err(2_147_483_648.0),
            over("signed integer is greater than maximum")
        );
        assert_eq!(
            err(-2_147_483_649.0),
            over("signed integer is less than minimum")
        );
        assert_eq!(
            err(9.223_372_036_854_776e18),
            over("Python int too large to convert to C long")
        );
        assert_eq!(
            err(-9.223_372_036_854_776e18),
            over("signed integer is less than minimum")
        );
        assert_eq!(
            err(1e300),
            over("Python int too large to convert to C long")
        );
    }
}
