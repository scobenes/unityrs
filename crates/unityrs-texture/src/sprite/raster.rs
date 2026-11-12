// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/Draw.c's ImagingDrawPolygon (its fill), add_edge, polygon_generic (for an 8-bit image, as polygon8 calls it), hline8, x_cmp, and the ROUND_UP and ROUND_DOWN macros, and src/_imaging.c's _draw_polygon's conversion of the vertices. See THIRD_PARTY_LICENSES.
//! A filled polygon on a mode `1` mask, as Pillow 9.5.0 draws it
//! (`ImageDraw.polygon(xy, fill=...)`, every mask `SpriteHelper` makes):
//! the rows' spans it fills, handed to the caller in the order Pillow
//! writes them, each already clipped to the mask, as `hline8` clips its
//! `memset`. A span is `(y, x0, x1)`, inclusive. A mask holds one value
//! (the fill), and every use of it here tests a pixel for non-zero
//! (`paste_mask_1`, `convert("L")` of a `1` image), so the spans are the
//! mask: a pixel is set when a span covers it.
//!
//! **The arithmetic** is Pillow's, in its C types: the vertices are
//! `int`s, truncated from the caller's `float`s (`_draw_polygon`:
//! `(int)xy[i]`, [`super::canvas::c_trunc`]); an edge's slope and each
//! scan line's crossings are `float`s (binary32), `(y - y0) * dx + x0`
//! with the `int`s converted first; the corner fix reads `fmax` and `fmin`
//! of them in `double` and stores the result back as a `float`; and a
//! crossing's span edge is `ROUND_UP` or `ROUND_DOWN`, whose positive arm
//! adds `0.5F` in `float` and whose negative arm adds it to `fabs`, a
//! `double`. The wheel's `_imaging` is built for x86-64's baseline, with no
//! FMA (no `vfmadd` in its disassembly), so nothing is contracted; Rust
//! contracts nothing either. `int` arithmetic wraps as GCC's does.
//! The crossings are sorted as `qsort` with `x_cmp` sorts them; two that
//! compare equal are equal floats, whose order changes no span.

// The C and Python names they port: `x0`, `a`, `b`, `xx`.
// Floats compared exactly, as the ported code compares them.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::float_cmp
)]

use std::cmp::Ordering;

use super::SpriteError;
use super::canvas::c_trunc;

/// An edge (`Edge`, `src/libImaging/Draw.c:56-62`): `d` is not read by the
/// fill.
#[derive(Clone, Copy, Debug, Default)]
struct Edge {
    x0: i32,
    y0: i32,
    xmin: i32,
    ymin: i32,
    xmax: i32,
    ymax: i32,
    dx: f32,
}

/// `add_edge` (`src/libImaging/Draw.c:587-616`).
fn edge(x0: i32, y0: i32, x1: i32, y1: i32) -> Edge {
    #[allow(clippy::cast_precision_loss)] // C's int-to-float conversion.
    let dx = if y0 == y1 {
        0.0
    } else {
        x1.wrapping_sub(x0) as f32 / y1.wrapping_sub(y0) as f32
    };
    Edge {
        x0,
        y0,
        xmin: x0.min(x1),
        ymin: y0.min(y1),
        xmax: x0.max(x1),
        ymax: y0.max(y1),
        dx,
    }
}

/// `(y - e.y0) * e.dx + e.x0`, in `float`.
#[allow(clippy::cast_precision_loss)]
fn cross(y: i32, e: &Edge) -> f32 {
    y.wrapping_sub(e.y0) as f32 * e.dx + e.x0 as f32
}

/// `ROUND_UP(f)` (`Draw.c:49`).
fn round_up(f: f32) -> i32 {
    if f >= 0.0 {
        c_trunc(f64::from(f + 0.5).floor())
    } else {
        c_trunc(-(f64::from(f).abs() + 0.5).floor())
    }
}

/// `ROUND_DOWN(f)` (`Draw.c:50`).
fn round_down(f: f32) -> i32 {
    if f >= 0.0 {
        c_trunc(f64::from(f - 0.5).ceil())
    } else {
        c_trunc(-(f64::from(f).abs() - 0.5).ceil())
    }
}

/// `x_cmp` (`Draw.c:391-401`): the sign of the `float` difference.
fn x_cmp(a: f32, b: f32) -> Ordering {
    let d = a - b;
    if d < 0.0 {
        Ordering::Less
    } else if d > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// The mask's size, and what each span is handed to.
pub(crate) struct Mask<'f> {
    pub(crate) xsize: i32,
    pub(crate) ysize: i32,
    pub(crate) span: &'f mut dyn FnMut(i32, i32, i32) -> Result<(), SpriteError>,
    /// Counts a scan line's work before it runs.
    pub(crate) work: &'f mut dyn FnMut(u64) -> Result<(), SpriteError>,
}

impl Mask<'_> {
    /// `hline8` (`Draw.c:99-122`): the span, clipped to the mask.
    fn hline(&mut self, mut x0: i32, y0: i32, mut x1: i32) -> Result<(), SpriteError> {
        if y0 < 0 || y0 >= self.ysize {
            return Ok(());
        }
        if x0 < 0 {
            x0 = 0;
        } else if x0 >= self.xsize {
            return Ok(());
        }
        if x1 < 0 {
            return Ok(());
        } else if x1 >= self.xsize {
            x1 = self.xsize - 1;
        }
        if x0 <= x1 {
            (self.span)(y0, x0, x1)?;
        }
        Ok(())
    }
}

/// At most three edges: every polygon `SpriteHelper` draws is a triangle.
#[derive(Default)]
struct Edges {
    e: [Edge; 3],
    n: usize,
}

impl Edges {
    fn push(&mut self, edge: Edge) {
        if let Some(slot) = self.e.get_mut(self.n) {
            *slot = edge;
            self.n += 1;
        }
    }

    fn last_mut(&mut self) -> Option<&mut Edge> {
        self.e.get_mut(self.n.checked_sub(1)?)
    }

    fn as_slice(&self) -> &[Edge] {
        self.e.get(..self.n).unwrap_or_default()
    }
}

/// `ImagingDrawPolygon(im, count, xy, ink, fill=1, ...)` (`Draw.c:769-813`): the
/// triangle through `xy`, filled on `mask`. Its edges and crossings live
/// on the stack (three edges, six crossings).
pub(crate) fn fill(xy: &[(i32, i32); 3], mask: &mut Mask<'_>) -> Result<(), SpriteError> {
    // A mask with no pixel has no span to fill. No renderer path draws on
    // one (each returns first for an empty canvas or crop, whose rows it
    // counts there); called with one, this steps through its rows as
    // Pillow's `polygon` does, each counted, with no special case.
    let count = xy.len();
    let mut e = Edges::default();
    let pt = |i: usize| xy.get(i).copied().unwrap_or_default();
    let mut i = 0;
    while i + 1 < count {
        let ((x0, y0), (x1, y1)) = (pt(i), pt(i + 1));
        if y0 == y1 && i != 0 && y0 == pt(i - 1).1 {
            // A horizontal line right after another.
            let prev_x = pt(i - 1).0;
            if let Some(last) = e.last_mut() {
                if x1 > x0 && x0 > prev_x {
                    last.xmax = x1;
                    i += 1;
                    continue;
                } else if x1 < x0 && x0 < prev_x {
                    last.xmin = x1;
                    i += 1;
                    continue;
                }
            }
        }
        e.push(edge(x0, y0, x1, y1));
        i += 1;
    }
    let (last, first) = (pt(i), pt(0));
    if last != first {
        e.push(edge(last.0, last.1, first.0, first.1));
    }
    polygon(e.as_slice(), mask)
}

/// `polygon_generic` for an 8-bit image (`Draw.c:436-570`, `hasAlpha` 0).
fn polygon(e: &[Edge], mask: &mut Mask<'_>) -> Result<(), SpriteError> {
    if e.is_empty() {
        return Ok(());
    }
    let mut table = Edges::default();
    let mut ymin = mask.ysize.wrapping_sub(1);
    let mut ymax = 0;
    for edge in e {
        ymin = ymin.min(edge.ymin);
        ymax = ymax.max(edge.ymax);
        if edge.ymin == edge.ymax {
            mask.hline(edge.xmin, edge.ymin, edge.xmax)?;
            continue;
        }
        table.push(*edge);
    }
    let table = table.as_slice();
    ymin = ymin.max(0);
    ymax = ymax.min(mask.ysize);
    // `calloc(edge_count * 2, sizeof(float))`, kept across scan lines.
    let mut store = [0.0_f32; 6];
    let xx = store.get_mut(..table.len() * 2).unwrap_or_default();
    // An inclusive range ends at `ymax` even where it is `INT_MAX`, where a
    // C `int` counter's `y++` would overflow.
    for y in ymin..=ymax {
        (mask.work)(table.len() as u64 + 1)?;
        let mut j = 0;
        for (i, cur) in table.iter().enumerate() {
            if y < cur.ymin || y > cur.ymax {
                continue;
            }
            let x = cross(y, cur);
            set(xx, j, x);
            j += 1;
            if y == cur.ymax && y < ymax {
                // Needed to draw consistent polygons.
                set(xx, j, x);
                j += 1;
            } else if cur.dx != 0.0 && x.round() == x {
                // Connect discontiguous corners.
                corner(table, i, cur, y, ymax, x, xx);
            }
        }
        let crossings = xx.get_mut(..j).unwrap_or_default();
        crossings.sort_unstable_by(|a, b| x_cmp(*a, *b));
        let mut k = 1;
        while k < j {
            let (a, b) = (get(xx, k - 1), get(xx, k));
            mask.hline(round_up(a), y, round_down(b))?;
            k += 2;
        }
    }
    Ok(())
}

/// The corner fix of `polygon_generic` (`Draw.c:496-531`): an earlier edge that
/// meets `cur` at `x` on this line moves the crossing stored at its own
/// index (Pillow indexes the crossings by the edge there).
fn corner(table: &[Edge], i: usize, cur: &Edge, y: i32, ymax: i32, x: f32, xx: &mut [f32]) {
    for (k, other) in table.iter().enumerate().take(i) {
        if (cur.dx > 0.0 && other.dx <= 0.0) || (cur.dx < 0.0 && other.dx >= 0.0) {
            continue;
        }
        let joins = (y == cur.ymin && y == other.ymin) || (y == cur.ymax && y == other.ymax);
        if joins && x == cross(y, other) {
            let offset = if y == ymax { -1 } else { 1 };
            let line = y.wrapping_add(offset);
            let adjacent = f64::from(cross(line, cur));
            let adjacent_other = f64::from(cross(line, other));
            #[allow(clippy::cast_possible_truncation)] // the C assignment to a float
            let v = if y == cur.ymax {
                if cur.dx > 0.0 {
                    (adjacent.max(adjacent_other) + 1.0) as f32
                } else {
                    (adjacent.min(adjacent_other) - 1.0) as f32
                }
            } else if cur.dx > 0.0 {
                adjacent.min(adjacent_other) as f32
            } else {
                (adjacent.max(adjacent_other) + 1.0) as f32
            };
            set(xx, k, v);
            break;
        }
    }
}

fn set(xx: &mut [f32], at: usize, v: f32) {
    if let Some(slot) = xx.get_mut(at) {
        *slot = v;
    }
}

fn get(xx: &[f32], at: usize) -> f32 {
    xx.get(at).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(xy: &[(i32, i32); 3], w: i32, h: i32) -> Vec<(i32, i32, i32)> {
        let mut out = Vec::new();
        let mut span = |y, a, b| {
            out.push((y, a, b));
            Ok(())
        };
        let mut work = |_| Ok(());
        fill(
            xy,
            &mut Mask {
                xsize: w,
                ysize: h,
                span: &mut span,
                work: &mut work,
            },
        )
        .unwrap();
        out
    }

    #[test]
    fn a_right_triangle_fills_its_rows() {
        let s = spans(&[(0, 0), (4, 0), (0, 4)], 8, 8);
        assert!(s.contains(&(0, 0, 4)));
        assert!(s.iter().all(|&(y, a, b)| (0..=4).contains(&y) && a <= b));
    }

    #[test]
    fn the_last_row_of_an_int_tall_mask_ends_the_loop() {
        let s = spans(
            &[(0, i32::MAX - 1), (2, i32::MAX), (0, i32::MAX)],
            4,
            i32::MAX,
        );
        assert_eq!(s.last().map(|&(y, ..)| y), Some(i32::MAX - 1));
        assert_eq!(spans(&[(0, 0), (4, 0), (0, 4)], 0, i32::MAX), []);
    }

    #[test]
    fn rounding_matches_pillow_s_macros() {
        assert_eq!((round_up(0.5), round_up(-0.5), round_up(1.49)), (1, -1, 1));
        assert_eq!(
            (round_down(0.5), round_down(-0.5), round_down(1.5)),
            (0, 0, 1)
        );
    }
}
