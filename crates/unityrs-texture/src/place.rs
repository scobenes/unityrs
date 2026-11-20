//! Writing a block-compressed image's blocks to their place: the blocks of
//! a padded image, block row by block row, each a square of 4 rows, into
//! the image's rows flipped top to bottom (UnityPy flips every decoded
//! image), with the pixels past its right and bottom edges dropped (its
//! crop). A block's row of pixels is written whole, as one copy of a
//! constant size, but at the right edge.

/// Writes the blocks of a `grid` (blocks across, blocks down) into `out`,
/// whose rows are `row` bytes, `h` of them, top to bottom: block row `by`
/// holds source rows `4 by` to `4 by + 3`, source row `y` is output row
/// `h - 1 - y`, and each block's row of pixels is `N` bytes, all of them
/// written but at the image's right edge, where as many as the row holds.
/// `block(bx, by, rows)` writes each block's rows of pixels, top to
/// bottom, and is called for every block, row by row, left to right; its
/// `false` stops the decode, as the end of the data does.
// Inlined into each decoder, so the state `block` keeps (the decoder's
// place in its data) is a register, not stored and reloaded for every
// block (measured).
#[allow(clippy::inline_always, reason = "measured: every block's loop")]
#[inline(always)]
pub(crate) fn put_blocks<const N: usize>(
    out: &mut [u8],
    row: usize,
    h: usize,
    (blocks_x, blocks_y): (usize, usize),
    mut block: impl FnMut(usize, usize, [&mut [u8; N]; 4]) -> bool,
) {
    if row == 0 {
        return;
    }
    for by in 0..blocks_y {
        let top = by * 4;
        let n = h.saturating_sub(top).min(4);
        let end = h.saturating_sub(top).saturating_mul(row);
        let region = out.get_mut(end.saturating_sub(n.saturating_mul(row))..end);
        // The region's rows, bottom up: source rows `top` onwards.
        let mut rows = region
            .into_iter()
            .flat_map(|r| r.chunks_exact_mut(row).rev());
        let (Some(r0), Some(r1), Some(r2), Some(r3)) =
            (rows.next(), rows.next(), rows.next(), rows.next())
        else {
            drop(rows);
            if !short_row(out, (row, n, end), (blocks_x, by), &mut block) {
                return;
            }
            continue;
        };
        let (c0, e0) = r0.as_chunks_mut::<N>();
        let (c1, e1) = r1.as_chunks_mut::<N>();
        let (c2, e2) = r2.as_chunks_mut::<N>();
        let (c3, e3) = r3.as_chunks_mut::<N>();
        let full = c0.len().min(blocks_x);
        // Four rows of `full` blocks each (all of them hold as many), so
        // the loop is one index over all four.
        let (Some(c0), Some(c1), Some(c2), Some(c3)) = (
            c0.get_mut(..full),
            c1.get_mut(..full),
            c2.get_mut(..full),
            c3.get_mut(..full),
        ) else {
            return;
        };
        let whole = c0.iter_mut().zip(c1).zip(c2).zip(c3);
        for (bx, (((d0, d1), d2), d3)) in whole.enumerate() {
            if !block(bx, by, [d0, d1, d2, d3]) {
                return;
            }
        }
        // The edge block, whose pixels past the right edge are dropped,
        // and any further block (none, for a grid of the image's width).
        for bx in full..blocks_x {
            let mut px = [[0; N]; 4];
            let [p0, p1, p2, p3] = &mut px;
            if !block(bx, by, [p0, p1, p2, p3]) {
                return;
            }
            if bx == full {
                for (dst, p) in [&mut *e0, &mut *e1, &mut *e2, &mut *e3]
                    .into_iter()
                    .zip(&px)
                {
                    put_edge(dst, p);
                }
            }
        }
    }
}

/// [`put_blocks`] for a decoder whose blocks are `data`'s, in order:
/// `block(src, rows)` writes the rows of the block `src`, and the end of
/// `data` stops the decode, as `block`'s `false` does. Each block row's
/// blocks are taken with its rows of pixels, by one index, so no place in
/// `data` is kept between blocks (measured).
#[allow(clippy::inline_always, reason = "measured: every block's loop")]
#[inline(always)]
pub(crate) fn put_block_data<const B: usize, const N: usize>(
    out: &mut [u8],
    row: usize,
    h: usize,
    (blocks_x, blocks_y): (usize, usize),
    data: &[[u8; B]],
    mut block: impl FnMut(&[u8; B], [&mut [u8; N]; 4]) -> bool,
) {
    if row == 0 {
        return;
    }
    for by in 0..blocks_y {
        // The block row's blocks: `blocks_x` of them, or as many as remain.
        let src = data.get(by.saturating_mul(blocks_x)..).unwrap_or_default();
        let src = src.get(..blocks_x).unwrap_or(src);
        let top = by * 4;
        let n = h.saturating_sub(top).min(4);
        let end = h.saturating_sub(top).saturating_mul(row);
        let region = out.get_mut(end.saturating_sub(n.saturating_mul(row))..end);
        // The region's rows, bottom up: source rows `top` onwards.
        let mut rows = region
            .into_iter()
            .flat_map(|r| r.chunks_exact_mut(row).rev());
        let (Some(r0), Some(r1), Some(r2), Some(r3)) =
            (rows.next(), rows.next(), rows.next(), rows.next())
        else {
            drop(rows);
            let mut each = |bx: usize, _: usize, rows: [&mut [u8; N]; 4]| {
                src.get(bx).is_some_and(|s| block(s, rows))
            };
            if !short_row(out, (row, n, end), (blocks_x, by), &mut each) {
                return;
            }
            continue;
        };
        let (c0, e0) = r0.as_chunks_mut::<N>();
        let (c1, e1) = r1.as_chunks_mut::<N>();
        let (c2, e2) = r2.as_chunks_mut::<N>();
        let (c3, e3) = r3.as_chunks_mut::<N>();
        let full = c0.len().min(blocks_x);
        let (Some(c0), Some(c1), Some(c2), Some(c3)) = (
            c0.get_mut(..full),
            c1.get_mut(..full),
            c2.get_mut(..full),
            c3.get_mut(..full),
        ) else {
            return;
        };
        let whole = src.iter().zip(c0.iter_mut().zip(c1).zip(c2).zip(c3));
        for (s, (((d0, d1), d2), d3)) in whole {
            if !block(s, [d0, d1, d2, d3]) {
                return;
            }
        }
        // The edge block, whose pixels past the right edge are dropped,
        // and any further block (none, for a grid of the image's width).
        for bx in full..blocks_x {
            let Some(s) = src.get(bx) else {
                return;
            };
            let mut px = [[0; N]; 4];
            let [p0, p1, p2, p3] = &mut px;
            if !block(s, [p0, p1, p2, p3]) {
                return;
            }
            if bx == full {
                for (dst, p) in [&mut *e0, &mut *e1, &mut *e2, &mut *e3]
                    .into_iter()
                    .zip(&px)
                {
                    put_edge(dst, p);
                }
            }
        }
        if src.len() < blocks_x {
            return;
        }
    }
}

/// [`put_blocks`]'s block row `by` where it has fewer than 4 rows, the
/// image's last (`n` of them, ending at byte `end` of `out`) or, where
/// `out` is short of the image (no caller's is), none.
#[cold]
#[inline(never)]
fn short_row<const N: usize>(
    out: &mut [u8],
    (row, n, end): (usize, usize, usize),
    (blocks_x, by): (usize, usize),
    block: &mut impl FnMut(usize, usize, [&mut [u8; N]; 4]) -> bool,
) -> bool {
    for bx in 0..blocks_x {
        let mut px = [[0; N]; 4];
        let [p0, p1, p2, p3] = &mut px;
        if !block(bx, by, [p0, p1, p2, p3]) {
            return false;
        }
        let at = bx.saturating_mul(N);
        let Some(region) = out.get_mut(end.saturating_sub(n.saturating_mul(row))..end) else {
            continue;
        };
        for (dst, p) in region.chunks_exact_mut(row).rev().zip(&px) {
            if let Some(dst) = dst.get_mut(at..) {
                put_edge(dst, p);
            }
        }
    }
    true
}

/// The first bytes of a block's row of pixels that `dst` holds.
#[inline(never)]
fn put_edge<const N: usize>(dst: &mut [u8], p: &[u8; N]) {
    let len = dst.len().min(N);
    if let (Some(dst), Some(src)) = (dst.get_mut(..len), p.get(..len)) {
        dst.copy_from_slice(src);
    }
}

/// Writes a block row's pixels into `dst`, their place in the image: all 4,
/// or at the image's right edge as many as `dst` holds. A whole row is a
/// copy of a constant size, which the compiler inlines; the edge's copy is
/// kept out of line, so the two are not merged into one call.
#[allow(clippy::inline_always, reason = "measured: every block row's write")]
#[inline(always)]
pub(crate) fn put_row(dst: &mut [u8], pixels: &[[u8; 4]; 4]) {
    let flat = pixels.as_flattened().as_array().unwrap_or(&[0; 16]);
    match <&mut [u8; 16]>::try_from(&mut *dst) {
        Ok(row) => *row = *flat,
        Err(_) => put_edge(dst, flat),
    }
}
