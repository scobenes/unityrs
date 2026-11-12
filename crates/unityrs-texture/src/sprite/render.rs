// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image_from_sprite, mask_sprite, render_sprite_mesh and copy_triangle. See THIRD_PARTY_LICENSES.
// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/Image.py's new (its size check), paste and putalpha, Image.composite, and src/libImaging/Paste.c's ImagingPaste (its clipping) with paste_mask_1. See THIRD_PARTY_LICENSES.
//! [`render`]: `get_image_from_sprite`'s steps (`super`'s module docs),
//! in its order, each check where UnityPy makes it.
//!
//! **Python's numbers.** A vertex's components are Python `float`s or
//! `int`s ([`Num`]); `min` and `max` keep the first of equal values and
//! skip what does not compare (a NaN), as Python's do; an `int` less an
//! `int` is exact, and times a `float` is the `int` rounded to a `float`
//! first. A rounded coordinate (`round`) is an integral `f64` while it is
//! a float's, and a uv coordinate of `int` components the exact product,
//! each an [`Int`]. `copy_triangle` (`export/SpriteHelper.py:227-301`)
//! compares their differences exactly. On the affine path `numpy` takes
//! the source corners as an array: within `[-2^63, 2^64)` each is converted
//! to the nearest `float64` (an `int64`, `uint64` or `float64` array, all
//! converted to `float64` by `solve`), and outside it the array is of
//! `object`s, which `solve` cannot cast ([`SpriteError::SolveCast`]).
//!
//! **The masks** are drawn as spans ([`super::raster`]) and applied as they
//! are drawn: a mask is set or not, and every span is applied the same
//! whichever span set it first. `mask_sprite`'s result is `RGBA`: an
//! `RGBA` crop composited over zeros (the crop's pixel where the mask is
//! set, zero elsewhere, `Image.composite`), an `RGB` crop given the mask
//! as its alpha (`putalpha`: 255 where set, 0 elsewhere, its colour kept
//! everywhere). `copy_triangle` pastes through its mask (`paste_mask_1`:
//! a whole pixel where the mask is set), clipped to the canvas
//! (`ImagingPaste`).

// The C and Python names they port: `x0`, `a`, `b`, `xx`.
// Floats compared exactly, as the ported code compares them.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::float_cmp
)]

use std::sync::Arc;

use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_meshdecode::{Component, Processed, VertexArray};

use super::affine::{Map, solve};
use super::canvas::{
    self, Canvas, Sheet, bpp, c_int, c_int_of, c_trunc, crop_size, py_round, width_cap,
};
use super::exact::Int;
use super::raster::{self, Mask};
use super::{PackingMode, Rotation, Settings, SpriteData, SpriteError, contained};
use crate::decode::Image;
use crate::format::Mode;
use unityrs_meshdecode::MeshError;

/// A Python number: a vertex component.
#[derive(Clone, Copy, Debug)]
enum Num {
    F(f64),
    I(i128),
}

impl Num {
    fn of(c: Component) -> Self {
        match c {
            Component::Int(i) => Self::I(i128::from(i)),
            // `Component` is non-exhaustive: every other form is a float.
            other => Self::F(other.as_f64()),
        }
    }

    /// `a < b`, as Python compares a `float` with a `float` or an `int`
    /// with an `int` (the tuples of one array are of one kind).
    fn lt(self, b: Self) -> bool {
        match (self, b) {
            (Self::F(a), Self::F(b)) => a < b,
            (Self::I(a), Self::I(b)) => a < b,
            #[allow(clippy::cast_precision_loss)]
            (Self::I(a), Self::F(b)) => (a as f64) < b,
            #[allow(clippy::cast_precision_loss)]
            (Self::F(a), Self::I(b)) => a < b as f64,
        }
    }

    fn eq(self, b: Self) -> bool {
        !self.lt(b) && !b.lt(self) && !self.is_nan() && !b.is_nan()
    }

    fn is_nan(self) -> bool {
        matches!(self, Self::F(f) if f.is_nan())
    }

    /// `(self - b) * f`, a `float`.
    #[allow(clippy::cast_precision_loss)]
    fn sub_mul(self, b: Self, f: f64) -> f64 {
        match (self, b) {
            (Self::I(a), Self::I(b)) => (a - b) as f64 * f,
            (Self::F(a), Self::F(b)) => (a - b) * f,
            (Self::I(a), Self::F(b)) => (a as f64 - b) * f,
            (Self::F(a), Self::I(b)) => (a - b as f64) * f,
        }
    }
}

/// Python's `min` (`!want_max`) or `max` of a non-empty run.
fn extreme(values: impl Iterator<Item = Num>, want_max: bool) -> Option<Num> {
    let mut best: Option<Num> = None;
    for v in values {
        best = Some(match best {
            None => v,
            Some(b) if (want_max && b.lt(v)) || (!want_max && v.lt(b)) => v,
            Some(b) => b,
        });
    }
    best
}

fn comp(a: &VertexArray, t: usize, c: usize) -> Num {
    a.component(t, c).map_or(Num::F(0.0), Num::of)
}

/// A tuple unpacked into `expected` names: [`SpriteError::Unpack`] unless
/// the array's tuples have that many.
fn unpack(a: &VertexArray, expected: usize) -> Result<(), SpriteError> {
    let d = a.dimension();
    if d == expected {
        Ok(())
    } else {
        Err(SpriteError::Unpack {
            expected,
            got: (d < expected).then_some(d),
        })
    }
}

/// [`super::render()`].
pub(super) fn render<'p>(
    sheet: &Sheet<'_>,
    data: &SpriteData,
    mesh: impl FnOnce() -> Result<Processed<'p>, MeshError>,
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    let crop_box = canvas::crop_box(data.texture_rect)?;
    // The crop is made here (`:90-97`), so Pillow's width cap on it comes
    // before the settings are read, and the cap on its turned size before
    // the mesh, on every path.
    let (crop_w, crop_h) = crop_size(crop_box);
    width_cap(crop_w)?;
    let settings = Settings::parse(data.settings_raw)?;
    if settings.packed && settings.packing_rotation == Rotation::Rotate90 {
        width_cap(crop_h)?;
    }
    let mut out = if settings.packing_mode == PackingMode::Tight {
        let processed = mesh()?;
        if uv_truthy(processed.uv(0), meter)? {
            mesh_render(sheet, &processed, data.pixels_to_units, meter)?
        } else {
            let crop = cropped(sheet, crop_box, settings, meter)?;
            mask(&crop, &processed, data.pixels_to_units, meter)?
        }
    } else {
        cropped(sheet, crop_box, settings, meter)?
    };
    out.flip_top_bottom(meter)?;
    Ok(out.into_image())
}

/// `mesh.m_UV0 and any(u or v for u, v in mesh.m_UV0)` (`:120`).
fn uv_truthy(uv: Option<&VertexArray>, meter: &Arc<Meter>) -> Result<bool, SpriteError> {
    let Some(uv) = uv.filter(|u| !u.is_empty()) else {
        return Ok(false);
    };
    unpack(uv, 2)?;
    meter.count_decode(uv.len() as u64)?;
    for t in 0..uv.len() {
        let truthy = |c| uv.component(t, c).is_some_and(Component::is_truthy);
        if truthy(0) || truthy(1) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The crop (`:90-97`), turned or flipped when packed (`:100-113`).
fn cropped(
    sheet: &Sheet<'_>,
    crop_box: [i32; 4],
    settings: Settings,
    meter: &Arc<Meter>,
) -> Result<Canvas, SpriteError> {
    let mut crop = canvas::crop(sheet, crop_box, meter)?;
    if settings.packed {
        match settings.packing_rotation {
            Rotation::None => {}
            Rotation::FlipHorizontal => crop.flip_left_right(meter)?,
            Rotation::FlipVertical => crop.flip_top_bottom(meter)?,
            Rotation::Rotate180 => crop.rotate_180(meter)?,
            Rotation::Rotate90 => crop = crop.rotate_270(meter)?,
        }
    }
    Ok(crop)
}

/// A charged buffer of `n` items, made by `f` in order; the first error
/// stops it.
fn collect<T>(
    n: usize,
    meter: &Arc<Meter>,
    mut f: impl FnMut(usize) -> Result<T, SpriteError>,
) -> Result<MeteredVec<T>, SpriteError> {
    meter.count_decode(n as u64)?;
    let mut v = MeteredVec::with_capacity(meter, n)?;
    for i in 0..n {
        let item = f(i)?;
        // Within the capacity reserved: no growth to charge.
        v.push(item)?;
    }
    Ok(v)
}

/// `mask_sprite` (`:130-170`).
fn mask(
    crop: &Canvas,
    mesh: &Processed<'_>,
    ppu: f64,
    meter: &Arc<Meter>,
) -> Result<Canvas, SpriteError> {
    let positions = mesh
        .vertices()
        .filter(|p| !p.is_empty())
        .ok_or(SpriteError::NoVertices)?;
    unpack(positions, 3)?;
    let n = positions.len();
    // The two `min` scans.
    meter.count_decode(2 * n as u64)?;
    let min_of = |c| extreme((0..n).map(|t| comp(positions, t, c)), false).unwrap_or(Num::F(0.0));
    let (min_x, min_y) = (min_of(0), min_of(1));
    let points = collect(n, meter, |t| {
        Ok((
            comp(positions, t, 0).sub_mul(min_x, ppu),
            comp(positions, t, 1).sub_mul(min_y, ppu),
        ))
    })?;
    let triangles = mesh.get_triangles(meter)?;
    // The comprehension's checks, before any triangle is drawn (`:147-155`).
    for sm in triangles.submeshes() {
        meter.count_decode(sm.len())?;
        for tri in sm.triangles() {
            if tri.iter().any(|&i| i as usize >= n) {
                return Err(SpriteError::Index { tuple: false });
            }
        }
        if !sm.cut().is_empty() {
            return Err(SpriteError::Unpack {
                expected: 3,
                got: Some(sm.cut().len()),
            });
        }
        if sm.empty() > 0 {
            return Err(SpriteError::Unpack {
                expected: 3,
                got: Some(0),
            });
        }
    }
    let (w, h) = (crop.width, crop.height);
    let mut out = Canvas::new(Mode::Rgba, w, h, meter)?;
    // The rows of the mask and of putalpha's or composite's pass, an empty
    // canvas's included.
    meter.count_decode(u64::from(h))?;
    if w == 0 || h == 0 {
        // No pixel to set: the mask draws nothing that shows.
        return Ok(out);
    }
    let rgba = crop.mode == Mode::Rgba;
    contained(|| -> Result<(), SpriteError> {
        if !rgba {
            // putalpha: the colour everywhere, alpha 0 until the mask sets it.
            for y in 0..h {
                for x in 0..w {
                    let p = crop.get(x, y);
                    if let Some(o) = out.get_mut(x, y).get_mut(..3) {
                        o.copy_from_slice(p.get(..3).unwrap_or(&[0; 3]));
                    }
                }
            }
        }
        let (xsize, ysize) = (
            i32::try_from(w).unwrap_or(i32::MAX),
            i32::try_from(h).unwrap_or(i32::MAX),
        );
        let mut span = |y: i32, x0: i32, x1: i32| -> Result<(), SpriteError> {
            meter.count_decode(u64::try_from(x1 - x0 + 1).unwrap_or(0))?;
            for x in x0..=x1 {
                let (x, y) = (x.cast_unsigned(), y.cast_unsigned());
                if rgba {
                    let p = crop.get(x, y);
                    out.get_mut(x, y).copy_from_slice(p);
                } else if let Some(a) = out.get_mut(x, y).get_mut(3) {
                    *a = 255;
                }
            }
            Ok(())
        };
        let mut work = |n: u64| meter.count_decode(n).map_err(SpriteError::from);
        for sm in triangles.submeshes() {
            for tri in sm.triangles() {
                let corner = |i: u32| {
                    let (x, y) = points
                        .as_slice()
                        .get(i as usize)
                        .copied()
                        .unwrap_or_default();
                    (c_trunc(x), c_trunc(y))
                };
                let xy = [corner(tri[0]), corner(tri[1]), corner(tri[2])];
                raster::fill(
                    &xy,
                    &mut Mask {
                        xsize,
                        ysize,
                        span: &mut span,
                        work: &mut work,
                    },
                )?;
            }
        }
        Ok(())
    })??;
    Ok(out)
}

/// `render_sprite_mesh` (`:173-224`).
fn mesh_render(
    sheet: &Sheet<'_>,
    mesh: &Processed<'_>,
    ppu: f64,
    meter: &Arc<Meter>,
) -> Result<Canvas, SpriteError> {
    let triangles = mesh.get_triangles(meter)?;
    let positions = mesh.vertices().filter(|p| !p.is_empty());
    let (Some(first), Some(positions)) = (triangles.submeshes().first(), positions) else {
        return Err(SpriteError::NoTriangles);
    };
    let uv = mesh
        .uv(0)
        .filter(|u| !u.is_empty())
        .ok_or(SpriteError::NoTriangles)?;
    if positions.dimension() < 3 {
        return Err(SpriteError::Index { tuple: true });
    }
    let n = positions.len();
    // The axis sets (three scans), then `min` and `max` of the two kept
    // (four).
    meter.count_decode(7 * n as u64)?;
    // The axis with one value, from the last (`:186-191`).
    let constant = |c: usize| {
        let first = comp(positions, 0, c);
        n == 1 || (0..n).all(|t| comp(positions, t, c).eq(first))
    };
    let dropped = (0..3)
        .rev()
        .find(|&c| constant(c))
        .ok_or(SpriteError::Sprite3d)?;
    let mut axes = (0..3).filter(|&c| c != dropped);
    let (ax, ay) = (axes.next().unwrap_or(0), axes.next().unwrap_or(1));
    let x_min = extreme((0..n).map(|t| comp(positions, t, ax)), false).unwrap_or(Num::F(0.0));
    let y_min = extreme((0..n).map(|t| comp(positions, t, ay)), false).unwrap_or(Num::F(0.0));
    let x_max = extreme((0..n).map(|t| comp(positions, t, ax)), true).unwrap_or(Num::F(0.0));
    let y_max = extreme((0..n).map(|t| comp(positions, t, ay)), true).unwrap_or(Num::F(0.0));
    let positions_abs = collect(n, meter, |t| {
        Ok([
            py_round(comp(positions, t, ax).sub_mul(x_min, ppu))?,
            py_round(comp(positions, t, ay).sub_mul(y_min, ppu))?,
        ])
    })?;
    let (sw, sh) = (sheet.width(), sheet.height());
    let uv_abs = collect(uv.len(), meter, |t| {
        let scaled = |c: usize, side: u32| -> Result<Int, SpriteError> {
            match comp(uv, t, c) {
                // An `int64` times a `u32`: within an `i128`.
                Num::I(i) => Ok(Int::I(i.saturating_mul(i128::from(side)))),
                Num::F(f) => Ok(Int::of_f64(py_round(f * f64::from(side))?)),
            }
        };
        Ok([scaled(0, sw)?, scaled(1, sh)?])
    })?;
    let size = [
        py_round(x_max.sub_mul(x_min, ppu))?,
        py_round(y_max.sub_mul(y_min, ppu))?,
    ];
    // `Image.new`'s checks (`_check_size`, then `core.fill`'s `(ii)`, then
    // Pillow's width cap and the pixel bound in [`Canvas::new`]).
    if size[0] < 0.0 || size[1] < 0.0 {
        return Err(SpriteError::NegativeSize);
    }
    let w = u32::try_from(c_int(size[0])?).unwrap_or(0);
    let h = u32::try_from(c_int(size[1])?).unwrap_or(0);
    let mut out = Canvas::new(sheet.mode(), w, h, meter)?;
    let tuples = first
        .triangles()
        .iter()
        .map(|t| &t[..])
        .chain((!first.cut().is_empty()).then_some(first.cut()))
        .chain((0..first.empty()).map(|_| &[][..]));
    for tri in tuples {
        meter.count_decode(1)?;
        let mut src = [[Int::I(0); 2]; 3];
        let mut dst = [[0.0; 2]; 3];
        for (k, &i) in tri.iter().enumerate() {
            let s = uv_abs
                .as_slice()
                .get(i as usize)
                .ok_or(SpriteError::Index { tuple: false })?;
            if let Some(slot) = src.get_mut(k) {
                *slot = *s;
            }
        }
        for (k, &i) in tri.iter().enumerate() {
            let d = positions_abs
                .as_slice()
                .get(i as usize)
                .ok_or(SpriteError::Index { tuple: false })?;
            if let Some(slot) = dst.get_mut(k) {
                *slot = *d;
            }
        }
        if tri.len() < 3 {
            return Err(SpriteError::Index { tuple: true });
        }
        copy_triangle(sheet, src, &mut out, dst, meter)?;
    }
    Ok(out)
}

/// A source corner as `numpy` converts it for `solve` (module docs): the
/// nearest `f64`, ties to even, within `[-2^63, 2^64)`.
fn to_float64(v: Int) -> Option<f64> {
    match v {
        #[allow(clippy::cast_precision_loss)] // the conversion's own rounding
        Int::I(i) if i >= i128::from(i64::MIN) && i <= i128::from(u64::MAX) => Some(i as f64),
        _ => None,
    }
}

/// `copy_triangle` (`:227-301`): `src` the uv corners, `dst` the canvas
/// corners (integral, inside the canvas's `int` sides).
fn copy_triangle(
    sheet: &Sheet<'_>,
    src: [[Int; 2]; 3],
    out: &mut Canvas,
    dst: [[f64; 2]; 3],
    meter: &Arc<Meter>,
) -> Result<(), SpriteError> {
    let d = dst.map(|p| p.map(Int::of_f64));
    // `None` is a difference `Int::sub` does not give, one of at least 2^73,
    // which no destination offset (a difference of two `int`s) equals.
    let off = |t: &[[Int; 2]; 3]| {
        [
            [t[1][0].sub(t[0][0]), t[1][1].sub(t[0][1])],
            [t[2][0].sub(t[0][0]), t[2][1].sub(t[0][1])],
        ]
    };
    if off(&src) == off(&d) {
        return copy_unchanged(sheet, &src, out, &d, meter);
    }
    let mut y = [0.0; 6];
    for (o, v) in y.iter_mut().zip([
        src[0][0], src[1][0], src[2][0], src[0][1], src[1][1], src[2][1],
    ]) {
        *o = to_float64(v).ok_or(SpriteError::SolveCast)?;
    }
    let m = [
        [dst[0][0], dst[0][1], 1.0, 0.0, 0.0, 0.0],
        [dst[1][0], dst[1][1], 1.0, 0.0, 0.0, 0.0],
        [dst[2][0], dst[2][1], 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, dst[0][0], dst[0][1], 1.0],
        [0.0, 0.0, 0.0, dst[1][0], dst[1][1], 1.0],
        [0.0, 0.0, 0.0, dst[2][0], dst[2][1], 1.0],
    ];
    // An exactly zero pivot is numpy's LinAlgError; the triangle copies
    // nothing (Tier C, `super`'s module docs).
    let Some(a) = solve(&m, &y) else {
        return Ok(());
    };
    // The rows of the transform's output, of the mask and of the paste,
    // each the canvas's size. This count is unreachable for an empty
    // canvas, and so is the return after it: on a canvas 0 wide every
    // destination x is 0 (each rounds a value no greater than the width
    // that rounds to 0), so the system's first column is zero and its first
    // pivot exactly zero, and on one 0 high its second column is, so
    // `solve` has returned `None` above. The empty canvas's rows are counted
    // where it is made and flipped (`Canvas::new`, `flip_top_bottom`), and
    // a translation's by `copy_unchanged`. The return stays as a guard.
    meter.count_decode(u64::from(out.height))?;
    if out.width == 0 || out.height == 0 {
        return Ok(());
    }
    let mut map = Map::new(a, out.width, out.height, sheet, meter)?;
    let xy = dst.map(|p| (c_trunc(p[0]), c_trunc(p[1])));
    let (xsize, ysize) = (
        i32::try_from(out.width).unwrap_or(i32::MAX),
        i32::try_from(out.height).unwrap_or(i32::MAX),
    );
    let b = bpp(out.mode);
    contained(|| -> Result<(), SpriteError> {
        let mut span = |y: i32, x0: i32, x1: i32| -> Result<(), SpriteError> {
            meter.count_decode(u64::try_from(x1 - x0 + 1).unwrap_or(0))?;
            map.sample(sheet, y, x0, x1, meter, &mut |x, p| {
                let o = out.get_mut(x.cast_unsigned(), y.cast_unsigned());
                match p {
                    Some(p) => o.copy_from_slice(p.get(..b).unwrap_or_default()),
                    None => o.fill(0),
                }
            })
        };
        let mut work = |n: u64| meter.count_decode(n).map_err(SpriteError::from);
        raster::fill(
            &xy,
            &mut Mask {
                xsize,
                ysize,
                span: &mut span,
                work: &mut work,
            },
        )
    })?
}

/// `copy_triangle`'s branch for offsets that match (`:248-269`): the
/// source's bounding box cropped, masked by the triangle, pasted at the
/// destination's upper left.
fn copy_unchanged(
    sheet: &Sheet<'_>,
    s: &[[Int; 2]; 3],
    out: &mut Canvas,
    d: &[[Int; 2]; 3],
    meter: &Arc<Meter>,
) -> Result<(), SpriteError> {
    type Axis = fn(&[Int; 2]) -> Int;
    let pick = |t: &[[Int; 2]; 3], at: Axis, max: bool| {
        let [first, rest @ ..] = t;
        let mut best = at(first);
        for p in rest {
            let ord = at(p).cmp(best);
            if (max && ord.is_gt()) || (!max && ord.is_lt()) {
                best = at(p);
            }
        }
        best
    };
    let (x, y): (Axis, Axis) = (|p| p[0], |p| p[1]);
    let (ul, lr) = (
        [pick(s, x, false), pick(s, y, false)],
        [pick(s, x, true), pick(s, y, true)],
    );
    // Image.crop's checks on the box of ints: the bomb check, then `(iiii)`.
    canvas::bomb_check([ul[0], ul[1], lr[0], lr[1]])?;
    let (x0, y0, x1, y1) = (
        c_int_of(ul[0])?,
        c_int_of(ul[1])?,
        c_int_of(lr[0])?,
        c_int_of(lr[1])?,
    );
    // ImagingCrop's sides, in `int`, and Pillow's width cap on the crop
    // (and on the mask of its size).
    let (cw, ch) = crop_size([x0, y0, x1, y1]);
    width_cap(cw)?;
    let (w, h) = (cw.cast_signed(), ch.cast_signed());
    // paste's box: (dx, dy, dx + w, dy + h) as `(iiii)`.
    let (dx, dy) = (c_int_of(pick(d, x, false))?, c_int_of(pick(d, y, false))?);
    c_int_of(Int::I(i128::from(dx) + i128::from(w)))?;
    c_int_of(Int::I(i128::from(dy) + i128::from(h)))?;
    // ImagingPaste's clipping (dx, dy >= 0 here).
    let (ow, oh) = (
        i32::try_from(out.width).unwrap_or(i32::MAX),
        i32::try_from(out.height).unwrap_or(i32::MAX),
    );
    let xsize = w.min(ow.saturating_sub(dx));
    let ysize = h.min(oh.saturating_sub(dy));
    // The rows of the crop and of its mask, counted where nothing is pasted
    // too.
    meter.count_decode(u64::from(ch))?;
    if xsize <= 0 || ysize <= 0 {
        return Ok(());
    }
    // The mask's corners: `(x - ul_x, y - ul_y)`, within an `int` here
    // (the box passed `(iiii)`), as `draw.polygon` takes them.
    let rel = |v: Int, origin: Int| {
        #[allow(clippy::cast_precision_loss)] // within an `int`: exact
        v.sub(origin).map_or(i32::MIN, |r| c_trunc(r as f64))
    };
    let xy = s.map(|[x, y]| (rel(x, ul[0]), rel(y, ul[1])));
    contained(|| -> Result<(), SpriteError> {
        let mut span = |y: i32, a: i32, b: i32| -> Result<(), SpriteError> {
            if y >= ysize {
                return Ok(());
            }
            let b = b.min(xsize - 1);
            meter.count_decode(u64::try_from(b - a + 1).unwrap_or(0))?;
            for x in a..=b {
                let p = sheet.pixel(i64::from(x0) + i64::from(x), i64::from(y0) + i64::from(y));
                let o = out.get_mut((dx + x).cast_unsigned(), (dy + y).cast_unsigned());
                match p {
                    Some(p) => o.copy_from_slice(p),
                    None => o.fill(0),
                }
            }
            Ok(())
        };
        let mut work = |n: u64| meter.count_decode(n).map_err(SpriteError::from);
        raster::fill(
            &xy,
            &mut Mask {
                xsize: w,
                ysize: h,
                span: &mut span,
                work: &mut work,
            },
        )
    })?
}
