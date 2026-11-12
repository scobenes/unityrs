// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's copy_triangle. See THIRD_PARTY_LICENSES.
// Ported from OpenBLAS 0.3.31 (BSD-3-Clause): the files whose order its Haswell build runs for numpy.linalg.solve of a 6 x 6 system, each under OpenBLAS's LICENSE (LICENSE-OPENBLAS) and its own notice: interface/lapack/gesv.c, lapack/getrf/getrf_single.c, lapack/getf2/getf2_k.c, lapack/getrs/getrs_single.c, lapack/laswp/generic/laswp_k_4.c (the DYNAMIC_ARCH build's laswp), driver/level2/trsv_L.c and trsv_U.c and kernel/x86_64/iamax_sse2.S under the next line's; kernel/x86_64/ddot.c, kernel/x86_64/dgemv_n_4.c with dgemv_n_microk_haswell-4.c and kernel/x86_64/daxpy.c under their 2014 notice (LICENSE-OPENBLAS-KERNELS); kernel/x86_64/dscal.c under its 2013 - 2015 notice (LICENSE-OPENBLAS-DSCAL). See THIRD_PARTY_LICENSES.
// Ported from GotoBLAS2 code in OpenBLAS 0.3.31 (BSD-2-Clause-Views): interface/lapack/gesv.c, lapack/getrf/getrf_single.c, lapack/getf2/getf2_k.c, lapack/getrs/getrs_single.c, lapack/laswp/generic/laswp_k_4.c, driver/level2/trsv_L.c and trsv_U.c, and kernel/x86_64/iamax_sse2.S, the files of the line above whose notice is The University of Texas at Austin's 2009-2010 licence (LICENSE-OPENBLAS-UT). See THIRD_PARTY_LICENSES.
// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/Geometry.c's ImagingTransformAffine, ImagingScaleAffine, check_fixed and affine_fixed, as Image.transform(size, Transform.AFFINE, data) calls them (NEAREST). See THIRD_PARTY_LICENSES.
//! The affine path of `copy_triangle` (`export/SpriteHelper.py:271-301`):
//! the 6 x 6 system `numpy.linalg.solve` solves for the map from the
//! destination triangle to the source, and Pillow's nearest-neighbour
//! affine transform of the sheet by it.
//!
//! **The solve** (`docs/plans/extract.md` section 3, item 6). `numpy`
//! 2.4.6 solves through its `OpenBLAS` 0.3.31 (`dgesv`): for a system this
//! small, `getrf_single` hands the whole matrix to `getf2`, a left-looking
//! LU with partial pivoting (`ddot` for the column's upper part, `dgemv_n`
//! for its lower, `idamax` for the pivot, a reciprocal and `dscal`), and
//! `getrs` applies the row swaps and two triangular solves (`trsv`, each an
//! `axpy` per column). Its floating-point order is the kernels': [`solve`]
//! follows them as the Haswell build (`OPENBLAS_CORETYPE=Haswell`, the
//! reference host's) runs them, from their source and the wheel's
//! disassembly, which has one fused multiply-add on this path, in
//! `dgemv_kernel_4x2` (`vfmadd231pd`), and none elsewhere (the kernels are
//! built with `-mavx2` and no `-mfma`). That one multiplies by `getf2`'s
//! alpha, -1, which is exact, so it rounds as the unfused step does and
//! no system tells them apart; it is written as `f64::mul_add` all the
//! same. [`solve`] itself is held to `numpy` under that pin bit for bit on
//! 2,000 seeded systems of `copy_triangle`'s shape, 116 of them singular
//! (`tests/sprite_solve.rs`, recorded by `tools/fixtures/sprite.py`), and
//! through the sprite oracle's cases that call solve. Before it, a Python model
//! of this port matched `numpy` under the pin in every bit of 50,000 random
//! integer systems of those shapes and differed from the wheel's default
//! kernels on 15,702 of them (`PARITY.md` section 15.2): the order is the
//! kernels', and this is theirs. An exactly zero pivot is `LinAlgError`
//! "Singular matrix"; [`solve`] returns `None` for it.
//!
//! **The transform** (`ImagingTransformAffine`): a map with no shear (`a[1]`
//! and `a[3]` both 0) is a scale, whose columns are tabulated once and rows
//! stepped (`ImagingScaleAffine`); a map whose four corners all lie within
//! 32,768 of the origin runs in 16.16 fixed point (`affine_fixed`); any
//! other in `double`, each pixel's coordinate the sum of the steps before
//! it. Each is ported in its arithmetic: the fixed-point steps are `int`
//! sums, which wrap as GCC's do, and a `double` truncated to an `int` is
//! `cvttsd2si`'s ([`c_trunc`]). The transform writes a pixel only where
//! its coordinate falls in the sheet, the rest zero, and `copy_triangle`
//! pastes it through the destination triangle's mask: [`Map::sample`]
//! answers for the masked pixels alone, from the same arithmetic, so the
//! transformed image is never made.

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
use super::canvas::{Sheet, c_trunc};

/// `DBL_MIN`, below which `getf2` neither swaps nor scales.
const DBL_MIN: f64 = f64::MIN_POSITIVE;

/// The order of 6.
const N: usize = 6;

/// `numpy.linalg.solve(m, y)` for `copy_triangle`'s system (module docs):
/// `m` by rows, as `copy_triangle` builds it. `None` where `numpy` raises
/// `LinAlgError` (an exactly zero pivot).
#[must_use]
pub fn solve(m: &[[f64; N]; N], y: &[f64; N]) -> Option<[f64; N]> {
    // Column major, as LAPACK holds it: `a[r + c * N]`.
    let mut a = [0.0_f64; N * N];
    for (r, row) in m.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let v = *v;
            put(&mut a, r + c * N, v);
        }
    }
    let mut b = *y;
    let ipiv = getf2(&mut a)?;
    // getrs: dlaswp_plus on the right-hand side, then trsv NLU and NUN.
    for (k, ip) in ipiv.iter().enumerate() {
        if *ip != k {
            b.swap(k, *ip);
        }
    }
    for i in 0..N {
        // trsv_NLU: daxpy of -b[i] over the column below the diagonal.
        let da = -at(&b, i);
        for r in i + 1..N {
            let v = at(&b, r) + da * at(&a, r + i * N);
            put(&mut b, r, v);
        }
    }
    for i in 0..N {
        // trsv_NUN: the diagonal divides, then daxpy over the rows above.
        let k = N - 1 - i;
        let v = at(&b, k) / at(&a, k + k * N);
        put(&mut b, k, v);
        let da = -at(&b, k);
        for r in 0..k {
            let v = at(&b, r) + da * at(&a, r + k * N);
            put(&mut b, r, v);
        }
    }
    Some(b)
}

/// `getf2_k`: the LU, in place; the pivots, 0-based, or `None` for a zero
/// pivot (`info` > 0).
fn getf2(a: &mut [f64; N * N]) -> Option<[usize; N]> {
    let mut ipiv = [0_usize; N];
    let mut singular = false;
    for j in 0..N {
        let c = j * N;
        // The earlier pivots, applied to this column (its laswp).
        for i in 0..j {
            let ip = at_usize(&ipiv, i);
            if ip != i {
                a.swap(c + i, c + ip);
            }
        }
        // The column's upper part: ddot against the rows of L above.
        for i in 1..j {
            let d = ddot(i, |k| at(a, i + k * N), |k| at(a, c + k));
            let v = at(a, c + i) - d;
            put(a, c + i, v);
        }
        // Its lower part: dgemv_n, alpha -1.
        gemv_n(a, j, N - j, j, c);
        // The pivot: idamax's first largest magnitude.
        let mut jp = j;
        let mut best = at(a, c + j).abs();
        for r in j + 1..N {
            let v = at(a, c + r).abs();
            if v > best {
                best = v;
                jp = r;
            }
        }
        put_usize(&mut ipiv, j, jp);
        let t = at(a, c + jp);
        if t == 0.0 {
            singular = true;
        } else if t.abs() >= DBL_MIN {
            let t = 1.0 / t;
            if jp != j {
                // dswap over the columns so far, this one included: getf2_k.c's SWAP_K
                // call. An exchange rounds nothing, so no kernel's order is ported for it.
                for k in 0..=j {
                    a.swap(j + k * N, jp + k * N);
                }
            }
            for r in j + 1..N {
                let v = t * at(a, c + r);
                put(a, c + r, v);
            }
        }
    }
    (!singular).then_some(ipiv)
}

/// `ddot_k` with a strided `x` (`kernel/x86_64/ddot.c`): groups of four
/// summed in two accumulators, then a sequential tail.
fn ddot(n: usize, x: impl Fn(usize) -> f64, y: impl Fn(usize) -> f64) -> f64 {
    let (mut t1, mut t2) = (0.0, 0.0);
    let n1 = n & !3;
    let mut i = 0;
    while i < n1 {
        let m1 = y(i) * x(i);
        let m2 = y(i + 1) * x(i + 1);
        let m3 = y(i + 2) * x(i + 2);
        let m4 = y(i + 3) * x(i + 3);
        t1 += m1 + m3;
        t2 += m2 + m4;
        i += 4;
    }
    while i < n {
        t1 += y(i) * x(i);
        i += 1;
    }
    t1 + t2
}

/// `dgemv_n` with alpha -1 (`kernel/x86_64/dgemv_n_4.c`): rows `row0..`
/// (`m` of them) of column `c`, less the rows' first `n` entries times the
/// column's first `n`. Rows in blocks of four with fewer than four columns
/// run the Haswell 4x2 kernel (its products summed, then fused into `y`)
/// and the SSE2 4x1 kernel; the rows past the last four, the generic C
/// tail (a sum of products, then `y += alpha * sum`).
fn gemv_n(a: &mut [f64; N * N], row0: usize, m: usize, n: usize, c: usize) {
    if m < 1 || n < 1 {
        return;
    }
    let alpha = -1.0_f64;
    let m3 = m & 3;
    let m1 = m & !3;
    let m2 = (m & 2047) - m3;
    let mut done = 0;
    // n < 4 always here: m >= 4 only when j <= 2.
    if m1 < 2048 && m2 != 0 {
        let mut k = 0;
        if n & 2 != 0 {
            let (x0, x1) = (at(a, k + c), at(a, k + 1 + c));
            for r in 0..m2 {
                let t = at(a, row0 + r + (k + 1) * N) * x1 + at(a, row0 + r + k * N) * x0;
                let y = at(a, row0 + r + c);
                let v = t.mul_add(alpha, y);
                put(a, row0 + r + c, v);
            }
            k += 2;
        }
        if n & 1 != 0 {
            let xa = at(a, k + c) * alpha;
            for r in 0..m2 {
                let y = at(a, row0 + r + c);
                let v = y + at(a, row0 + r + k * N) * xa;
                put(a, row0 + r + c, v);
            }
        }
        done = m2;
    }
    let mut temps = [0.0_f64; 3];
    for k in 0..n {
        for (r, t) in temps.iter_mut().enumerate().take(m3) {
            *t += at(a, row0 + done + r + k * N) * at(a, k + c);
        }
    }
    for (r, t) in temps.iter().enumerate().take(m3) {
        let at_r = row0 + done + r + c;
        let v = at(a, at_r) + alpha * t;
        put(a, at_r, v);
    }
}

fn at(a: &[f64], i: usize) -> f64 {
    a.get(i).copied().unwrap_or_default()
}

fn put(a: &mut [f64], i: usize, v: f64) {
    if let Some(s) = a.get_mut(i) {
        *s = v;
    }
}

fn at_usize(a: &[usize], i: usize) -> usize {
    a.get(i).copied().unwrap_or_default()
}

fn put_usize(a: &mut [usize], i: usize, v: usize) {
    if let Some(s) = a.get_mut(i) {
        *s = v;
    }
}

/// `COORD(v)` (`Geometry.c:11`).
fn coord(v: f64) -> i32 {
    if v < 0.0 { -1 } else { c_trunc(v) }
}

/// `FLOOR(v)` (`Geometry.c:12`).
fn floor_int(v: f64) -> i32 {
    if v < 0.0 {
        c_trunc(v.floor())
    } else {
        c_trunc(v)
    }
}

/// `FIX(v)` (`Geometry.c:969`).
fn fix(v: f64) -> i32 {
    floor_int(v * 65536.0 + 0.5)
}

/// `check_fixed(a, x, y)` (`Geometry.c:936-940`).
fn check_fixed(a: &[f64; 6], x: i32, y: i32) -> bool {
    let (x, y) = (f64::from(x), f64::from(y));
    (x * a[0] + y * a[1] + a[2]).abs() < 32768.0 && (x * a[3] + y * a[4] + a[5]).abs() < 32768.0
}

/// The transform's arithmetic (module docs), for an output of `width` x
/// `height` (the box `(0, 0, width, height)`).
#[derive(Debug)]
enum Path {
    /// `ImagingScaleAffine`: the column table and its bounds, and the rows'
    /// start `yo` and step.
    Scale {
        xintab: Vec<i32>,
        xmin: i32,
        xmax: i32,
        yo: f64,
        a4: f64,
        // After the table: the charge outlives the buffer it pays for.
        _charge: Charge,
    },
    /// `affine_fixed`: the 16.16 steps.
    Fixed {
        a0: i32,
        a1: i32,
        a2: i32,
        a3: i32,
        a4: i32,
        a5: i32,
    },
    /// The `double` loop's start and steps.
    Float { xo: f64, yo: f64, a: [f64; 6] },
}

/// `Image.transform(size, Transform.AFFINE, a)` of the sheet, answered
/// pixel by pixel (module docs).
#[derive(Debug)]
pub(crate) struct Map {
    path: Path,
    /// A cursor over the rows: the row `row` and its start, kept so the
    /// rows a mask visits in order are each stepped to once.
    row: i32,
    row_x: f64,
    row_y: f64,
}

impl Map {
    /// The transform's path for `a` over an output of `width` x `height`
    /// and its set-up (the scale's column table, charged to `meter`).
    pub(crate) fn new(
        a: [f64; 6],
        width: u32,
        height: u32,
        sheet: &Sheet<'_>,
        meter: &Arc<Meter>,
    ) -> Result<Self, SpriteError> {
        let w = i32::try_from(width).map_err(|_| SpriteError::TooLarge { what: "canvas" })?;
        let h = i32::try_from(height).map_err(|_| SpriteError::TooLarge { what: "canvas" })?;
        let path = if a[1] == 0.0 && a[3] == 0.0 {
            meter.count_decode(u64::from(width))?;
            let (mut xintab, charge) =
                MeteredVec::<i32>::with_capacity(meter, width as usize)?.into_parts();
            // Within the capacity just reserved: calloc's zeros.
            xintab.resize(width as usize, 0);
            let sw = i32::try_from(sheet.width()).unwrap_or(i32::MAX);
            let mut xo = a[2] + a[0] * 0.5;
            let (mut xmin, mut xmax) = (w, 0);
            for (x, slot) in (0..w).zip(xintab.iter_mut()) {
                let xin = coord(xo);
                if xin >= 0 && xin < sw {
                    xmax = x + 1;
                    if x < xmin {
                        xmin = x;
                    }
                    *slot = xin;
                }
                xo += a[0];
            }
            Path::Scale {
                xintab,
                xmin,
                xmax,
                yo: a[5] + a[4] * 0.5,
                a4: a[4],
                _charge: charge,
            }
        } else if check_fixed(&a, 0, 0)
            && check_fixed(&a, w, h)
            && check_fixed(&a, 0, h)
            && check_fixed(&a, w, 0)
        {
            Path::Fixed {
                a0: fix(a[0]),
                a1: fix(a[1]),
                a3: fix(a[3]),
                a4: fix(a[4]),
                a2: fix(a[2] + a[0] * 0.5 + a[1] * 0.5),
                a5: fix(a[5] + a[3] * 0.5 + a[4] * 0.5),
            }
        } else {
            Path::Float {
                xo: a[2] + a[1] * 0.5 + a[0] * 0.5,
                yo: a[5] + a[4] * 0.5 + a[3] * 0.5,
                a,
            }
        };
        let (row_x, row_y) = match &path {
            Path::Scale { yo, .. } => (0.0, *yo),
            Path::Float { xo, yo, .. } => (*xo, *yo),
            Path::Fixed { .. } => (0.0, 0.0),
        };
        Ok(Self {
            path,
            row: 0,
            row_x,
            row_y,
        })
    }

    /// Steps the row cursor to `y`, from the first row when `y` is behind
    /// it, counting each step's work.
    fn seek(&mut self, y: i32, meter: &Arc<Meter>) -> Result<(), SpriteError> {
        let (sx, sy, x0, y0) = match &self.path {
            Path::Scale { a4, yo, .. } => (0.0, *a4, 0.0, *yo),
            Path::Float { a, xo, yo } => (a[1], a[4], *xo, *yo),
            Path::Fixed { .. } => return Ok(()),
        };
        if y < self.row {
            self.row = 0;
            self.row_x = x0;
            self.row_y = y0;
        }
        meter.count_decode(u64::try_from(y - self.row).unwrap_or(0))?;
        while self.row < y {
            self.row_x += sx;
            self.row_y += sy;
            self.row += 1;
        }
        Ok(())
    }

    /// The transformed image's pixels `x0..=x1` of row `y`, each handed to
    /// `out` with its source pixel, `None` where the transform leaves it
    /// zero. `y` and the span lie inside the output.
    pub(crate) fn sample(
        &mut self,
        sheet: &Sheet<'_>,
        y: i32,
        x0: i32,
        x1: i32,
        meter: &Arc<Meter>,
        out: &mut dyn FnMut(i32, Option<&[u8]>),
    ) -> Result<(), SpriteError> {
        self.seek(y, meter)?;
        let (sw, sh) = (
            i32::try_from(sheet.width()).unwrap_or(i32::MAX),
            i32::try_from(sheet.height()).unwrap_or(i32::MAX),
        );
        let inside = |xin: i32, yin: i32| {
            (xin >= 0 && xin < sw && yin >= 0 && yin < sh).then_some((xin, yin))
        };
        match &self.path {
            Path::Scale {
                xintab, xmin, xmax, ..
            } => {
                let yi = coord(self.row_y);
                for x in x0..=x1 {
                    // Within [xmin, xmax) a column with no source keeps
                    // calloc's 0 (`Geometry.c:886-895`).
                    let hit = (yi >= 0 && yi < sh && x >= *xmin && x < *xmax).then(|| {
                        (
                            xintab.get(x.cast_unsigned() as usize).copied().unwrap_or(0),
                            yi,
                        )
                    });
                    out(
                        x,
                        hit.and_then(|(sx, sy)| sheet.pixel(i64::from(sx), i64::from(sy))),
                    );
                }
            }
            Path::Fixed {
                a0,
                a1,
                a2,
                a3,
                a4,
                a5,
            } => {
                let xx_row = a2.wrapping_add(a1.wrapping_mul(y));
                let yy_row = a5.wrapping_add(a4.wrapping_mul(y));
                for x in x0..=x1 {
                    let xx = xx_row.wrapping_add(a0.wrapping_mul(x));
                    let yy = yy_row.wrapping_add(a3.wrapping_mul(x));
                    let hit = inside(xx >> 16, yy >> 16);
                    out(
                        x,
                        hit.and_then(|(sx, sy)| sheet.pixel(i64::from(sx), i64::from(sy))),
                    );
                }
            }
            Path::Float { a, .. } => {
                meter.count_decode(u64::try_from(x0).unwrap_or(0))?;
                let (mut xx, mut yy) = (self.row_x, self.row_y);
                for _ in 0..x0 {
                    xx += a[0];
                    yy += a[3];
                }
                for x in x0..=x1 {
                    let hit = inside(coord(xx), coord(yy));
                    out(
                        x,
                        hit.and_then(|(sx, sy)| sheet.pixel(i64::from(sx), i64::from(sy))),
                    );
                    xx += a[0];
                    yy += a[3];
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_translation() {
        let dst = [(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)];
        let src = [(2.0, 3.0), (6.0, 3.0), (2.0, 7.0)];
        let m = [
            [dst[0].0, dst[0].1, 1.0, 0.0, 0.0, 0.0],
            [dst[1].0, dst[1].1, 1.0, 0.0, 0.0, 0.0],
            [dst[2].0, dst[2].1, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, dst[0].0, dst[0].1, 1.0],
            [0.0, 0.0, 0.0, dst[1].0, dst[1].1, 1.0],
            [0.0, 0.0, 0.0, dst[2].0, dst[2].1, 1.0],
        ];
        let y = [src[0].0, src[1].0, src[2].0, src[0].1, src[1].1, src[2].1];
        assert_eq!(solve(&m, &y), Some([1.0, 0.0, 2.0, 0.0, 1.0, 3.0]));
    }

    fn system(dst: [(f64, f64); 3]) -> [[f64; 6]; 6] {
        let row = |(x, y): (f64, f64), left: bool| {
            if left {
                [x, y, 1.0, 0.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 0.0, x, y, 1.0]
            }
        };
        [
            row(dst[0], true),
            row(dst[1], true),
            row(dst[2], true),
            row(dst[0], false),
            row(dst[1], false),
            row(dst[2], false),
        ]
    }

    /// Singular is an exactly zero pivot in the LU, which these collinear
    /// corners meet and those meet not: rounding leaves every pivot of
    /// (1, 1), (3, 2), (5, 3) non-zero, so numpy solves it, as the oracle's
    /// `path_collinear_a` records.
    #[test]
    fn singular_is_an_exactly_zero_pivot() {
        let zero_pivot = system([(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);
        assert_eq!(solve(&zero_pivot, &[0.0; 6]), None);
        for dst in [
            [(1.0, 1.0), (3.0, 2.0), (5.0, 3.0)],
            [(1.0, 0.0), (4.0, 1.0), (7.0, 2.0)],
        ] {
            assert!(
                solve(&system(dst), &[0.0, 1.0, 2.0, 0.0, 1.0, 3.0]).is_some(),
                "{dst:?}"
            );
        }
    }

    #[test]
    fn the_path_is_pillow_s() {
        use crate::{Format, decode};
        let meter = Meter::unlimited();
        let texture = decode(Format::Rgb24, &[0; 12], 2, 2, &meter).unwrap();
        let sheet = Sheet::new(&texture, None, &meter).unwrap();
        let path = |a: [f64; 6]| match Map::new(a, 4, 4, &sheet, &meter).unwrap().path {
            Path::Scale { .. } => "scale",
            Path::Fixed { .. } => "fixed",
            Path::Float { .. } => "float",
        };
        assert_eq!(path([2.0, 0.0, 1.0, 0.0, 2.0, 1.0]), "scale");
        assert_eq!(path([2.0, 0.0, 1.0, 1.0, 2.0, 1.0]), "fixed");
        // A corner at 4 * 8191.75 + 1 = 32768: past fixed point's range.
        assert_eq!(path([8191.75, 0.0, 1.0, 1.0, 2.0, 1.0]), "float");
        assert_eq!(path([8191.5, 0.0, 1.0, 1.0, 2.0, 1.0]), "fixed");
        assert_eq!(path([f64::NAN, 1.0, 0.0, 1.0, 1.0, 0.0]), "float");
    }
}
