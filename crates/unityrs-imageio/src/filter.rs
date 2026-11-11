// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/ZipEncode.c's PNG filter choice (ImagingZipEncode, the ZIP_PNG branch). See THIRD_PARTY_LICENSES.
//! The filter Pillow 9.5.0 picks for each row of a PNG, and the filtered row
//! (`ZipEncode.c:172-273`).
//!
//! Each row is tried with no filter, then up, then sub, then Paeth, while
//! the best so far has a non-zero sum; a later filter wins only with a
//! sum strictly below the best, so a tie keeps the earlier one. A row's sum
//! adds, for each filtered byte `v`, `v` below 128 and `256 - v` from 128:
//! its distance from zero taken as signed. Average is tried only with
//! `optimize`, which Pillow's defaults leave off.
//!
//! Pillow's sums are C `int`s, and so are these (`i32`), wrapping as
//! two's-complement `int` arithmetic does. A sum passes 2^31 - 1 only in a
//! row of 2^24 bytes (16 MiB) or more, each byte adding at most 128: an
//! `L` row 16,777,216 pixels wide, an `RGBA` one 4,194,304, both within
//! what Pillow encodes. There C leaves the overflow undefined, so such a
//! row's choice rests on the oracle's build wrapping it as two's
//! complement (a compiler may instead take a sum of non-negative terms to
//! be non-negative); no fixture holds such a row, and one would show only
//! the bytes it held, not what the build does with others.

/// The filter types (PNG's numbering), each a row's first byte.
const NONE: u8 = 0;
const SUB: u8 = 1;
const UP: u8 = 2;
const PAETH: u8 = 4;

/// The five rows Pillow's encoder keeps, each a filter byte and the row's
/// bytes: the row being encoded (`state->buffer`), the one before it
/// (`context->previous`, zeros before the first), and the candidates.
pub(crate) struct Rows {
    pub(crate) current: Vec<u8>,
    pub(crate) previous: Vec<u8>,
    up: Vec<u8>,
    sub: Vec<u8>,
    paeth: Vec<u8>,
    /// Bytes a pixel.
    bpp: usize,
}

/// The row a filter choice left to deflate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chosen {
    None,
    Up,
    Sub,
    Paeth,
}

impl Rows {
    /// The rows from five buffers of `row + 1` zero bytes each (allocated
    /// by the caller, charged first), for pixels of `bpp` bytes.
    pub(crate) fn new(buffers: [Vec<u8>; 5], bpp: usize) -> Self {
        let [mut current, previous, mut up, mut sub, mut paeth] = buffers;
        // ZipEncode.c:65-69: each buffer's first byte is its filter type;
        // the previous row starts black (:72).
        for (row, kind) in [
            (&mut current, NONE),
            (&mut up, UP),
            (&mut sub, SUB),
            (&mut paeth, PAETH),
        ] {
            if let Some(first) = row.first_mut() {
                *first = kind;
            }
        }
        Self {
            current,
            previous,
            up,
            sub,
            paeth,
            bpp,
        }
    }

    /// The row bytes of [`Self::current`], after its filter byte, to fill.
    pub(crate) fn current_pixels(&mut self) -> &mut [u8] {
        self.current.get_mut(1..).unwrap_or_default()
    }

    /// The bytes of `chosen`, filter byte first.
    pub(crate) fn chosen(&self, chosen: Chosen) -> &[u8] {
        match chosen {
            Chosen::None => &self.current,
            Chosen::Up => &self.up,
            Chosen::Sub => &self.sub,
            Chosen::Paeth => &self.paeth,
        }
    }

    /// The row just encoded becomes the one before the next
    /// (`ZipEncode.c:300-303`: the two buffers swap).
    pub(crate) fn advance(&mut self) {
        std::mem::swap(&mut self.current, &mut self.previous);
    }

    /// Picks the filter for [`Self::current`], filling the candidates it
    /// tries (`ZipEncode.c:180-272`).
    pub(crate) fn choose(&mut self) -> Chosen {
        let bpp = self.bpp;
        let row = || self.current.iter().skip(1);
        // 0. No filter.
        let mut sum = row().fold(0_i32, |s, &v| s.wrapping_add(distance(v)));
        let mut chosen = Chosen::None;

        // 2. Up, tried first to save time when a row repeats the last.
        if sum > 0 {
            let mut s = 0_i32;
            for ((out, &x), &above) in self
                .up
                .iter_mut()
                .skip(1)
                .zip(self.current.iter().skip(1))
                .zip(self.previous.iter().skip(1))
            {
                *out = x.wrapping_sub(above);
                s = s.wrapping_add(distance(*out));
            }
            if s < sum {
                chosen = Chosen::Up;
                sum = s;
            }
        }

        // 1. Sub ("prior" in ZipEncode.c): the first pixel as it is.
        if sum > 0 {
            let mut s = 0_i32;
            let (head, tail) = split(&mut self.sub, bpp);
            for (out, &x) in head.iter_mut().zip(self.current.iter().skip(1)) {
                *out = x;
                s = s.wrapping_add(distance(x));
            }
            for ((out, &x), &left) in tail
                .iter_mut()
                .zip(self.current.iter().skip(1 + bpp))
                .zip(self.current.iter().skip(1))
            {
                *out = x.wrapping_sub(left);
                s = s.wrapping_add(distance(*out));
            }
            if s < sum {
                chosen = Chosen::Sub;
                sum = s;
            }
        }

        // 4. Paeth: the first pixel's predictor is the byte above.
        if sum > 0 {
            let mut s = 0_i32;
            let (head, tail) = split(&mut self.paeth, bpp);
            for ((out, &x), &above) in head
                .iter_mut()
                .zip(self.current.iter().skip(1))
                .zip(self.previous.iter().skip(1))
            {
                *out = x.wrapping_sub(above);
                s = s.wrapping_add(distance(*out));
            }
            let lefts = self.current.iter().skip(1);
            let aboves = self.previous.iter().skip(1 + bpp);
            let corners = self.previous.iter().skip(1);
            for ((((out, &x), &a), &b), &c) in tail
                .iter_mut()
                .zip(self.current.iter().skip(1 + bpp))
                .zip(lefts)
                .zip(aboves)
                .zip(corners)
            {
                *out = x.wrapping_sub(paeth(a, b, c));
                s = s.wrapping_add(distance(*out));
            }
            if s < sum {
                chosen = Chosen::Paeth;
            }
        }
        chosen
    }
}

/// A filtered byte's distance from zero, taken as signed
/// (`(v < 128) ? v : 256 - v`).
#[inline]
fn distance(v: u8) -> i32 {
    if v < 128 {
        i32::from(v)
    } else {
        256 - i32::from(v)
    }
}

/// Pillow's Paeth predictor (`ZipEncode.c:251-264`): of the left (`a`),
/// above (`b`) and upper-left (`c`) bytes, the one nearest `a + b - c`,
/// `a` first on a tie, then `b`.
#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let pa = (ib - ic).abs();
    let pb = (ia - ic).abs();
    let pc = (ia + ib - 2 * ic).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// A candidate row's bytes after its filter byte, split at the first
/// pixel's end.
fn split(row: &mut [u8], bpp: usize) -> (&mut [u8], &mut [u8]) {
    let body = row.get_mut(1..).unwrap_or_default();
    let at = bpp.min(body.len());
    body.split_at_mut(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(width: usize, bpp: usize) -> Rows {
        let n = width * bpp + 1;
        Rows::new(std::array::from_fn(|_| vec![0; n]), bpp)
    }

    #[test]
    fn a_flat_first_row_is_left_unfiltered_only_when_it_is_zero() {
        let mut r = rows(4, 1);
        assert_eq!(r.choose(), Chosen::None);
        r.current_pixels().copy_from_slice(&[9, 9, 9, 9]);
        // Up leaves 9s too (the row before is black); sub leaves 9, 0, 0, 0.
        assert_eq!(r.choose(), Chosen::Sub);
        assert_eq!(r.chosen(Chosen::Sub), [SUB, 9, 0, 0, 0]);
    }

    #[test]
    fn a_repeated_row_is_up_and_a_tie_keeps_the_earlier_filter() {
        let mut r = rows(3, 1);
        r.current_pixels().copy_from_slice(&[1, 200, 3]);
        let _ = r.choose();
        r.advance();
        r.current_pixels().copy_from_slice(&[1, 200, 3]);
        assert_eq!(r.choose(), Chosen::Up);
        // A row whose sub and Paeth sums equal up's keeps up.
        let mut r = rows(2, 1);
        r.current_pixels().copy_from_slice(&[0, 1]);
        // none 1, up 1, sub 1, Paeth 1: none.
        assert_eq!(r.choose(), Chosen::None);
    }

    #[test]
    fn paeth_prefers_left_then_above() {
        assert_eq!(paeth(10, 10, 10), 10);
        // p = a + b - c = 20, which is a: pa = |b - c| = 0.
        assert_eq!(paeth(20, 10, 10), 20);
        // p = 20, which is b: pa = 10, pb = |a - c| = 0.
        assert_eq!(paeth(10, 20, 10), 20);
        // pa = pb = 5, pc = 0: the corner.
        assert_eq!(paeth(0, 10, 5), 5);
        // pa = pb = 5, pc = 10: the left.
        assert_eq!(paeth(5, 5, 0), 5);
    }

    #[test]
    fn distances_take_bytes_as_signed() {
        assert_eq!(distance(0), 0);
        assert_eq!(distance(127), 127);
        assert_eq!(distance(128), 128);
        assert_eq!(distance(255), 1);
    }
}
