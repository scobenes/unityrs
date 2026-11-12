//! Python's integers where a sprite's numbers pass an `i128`: a crop
//! edge or a uv corner is the `int` that `round()` makes of a `float`, which
//! can be as large as a `float` (about 2^1024), and Pillow's
//! decompression-bomb message names the product of two of their
//! differences exactly (`src/PIL/Image.py:3162-3180`).
//!
//! [`Int`] holds such an integer as an `i128`, or as the integral `f64` it
//! came from where it is past one; [`Mag`] is an exact magnitude, wide
//! enough for the product of two differences of `f64`s, on the stack.

use std::cmp::Ordering;
use std::fmt::Write;

/// A Python `int` made by `round()` of a `float`, or from integer
/// arithmetic within an `i128` (a uv component times a sheet's side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Int {
    /// Within an `i128`.
    I(i128),
    /// An integral `f64` of magnitude 2^126 or more: exact as the `f64`.
    F(f64),
}

/// 2^126: below it an integral `f64` is held as an `i128`.
const SMALL: f64 = 8.507_059_173_023_462e37;

impl Int {
    /// The integer of an integral, finite `f64`.
    #[allow(clippy::cast_possible_truncation)] // below 2^126: exact
    pub(crate) fn of_f64(v: f64) -> Self {
        if v.abs() < SMALL {
            Self::I(v as i128)
        } else {
            Self::F(v)
        }
    }

    /// Python's comparison of two `int`s.
    pub(crate) fn cmp(self, b: Self) -> Ordering {
        match (self, b) {
            (Self::I(a), Self::I(b)) => a.cmp(&b),
            // Both integral and finite: no NaN.
            (Self::F(a), Self::F(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
            // An `F` is past every `I`, on its own side of 0.
            (Self::I(_), Self::F(f)) => {
                if f > 0.0 {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (Self::F(f), Self::I(_)) => {
                if f > 0.0 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
        }
    }

    /// `self - b` of two `I`s, where it is within an `i128`, and of two
    /// equal `F`s; `None` otherwise, where the difference is at least 2^73,
    /// which no difference of two `i32`s equals.
    pub(crate) fn sub(self, b: Self) -> Option<i128> {
        match (self, b) {
            (Self::I(a), Self::I(b)) => a.checked_sub(b),
            // Two `f64`s of magnitude 2^126 or more are multiples of 2^73,
            // so they differ by 0 or by at least 2^73; an `F` beside an `I`
            // (below 2^126: `of_f64`'s, or a uv component times a sheet's
            // side, below 2^78) differs from it by at least 2^73 too, the
            // `f64` spacing below 2^126.
            (Self::F(a), Self::F(b)) if a.total_cmp(&b).is_eq() => Some(0),
            _ => None,
        }
    }

    /// `|self - b|`, exactly.
    pub(crate) fn abs_diff(self, b: Self) -> Mag {
        let (sa, ma) = self.parts();
        let (sb, mb) = b.parts();
        if sa == sb {
            if ma.cmp(&mb) == Ordering::Less {
                mb.sub(&ma)
            } else {
                ma.sub(&mb)
            }
        } else {
            ma.add(&mb)
        }
    }

    /// Its sign (`true` below 0) and magnitude.
    fn parts(self) -> (bool, Mag) {
        match self {
            Self::I(i) => (i < 0, Mag::from_u128(i.unsigned_abs())),
            Self::F(f) => (f < 0.0, Mag::from_f64(f.abs())),
        }
    }
}

/// The limbs of a [`Mag`]: 64 x 34 bits, past the 2,050 that the product
/// of two differences of finite `f64`s can take.
const LIMBS: usize = 34;

/// An exact non-negative integer, little-endian in 64-bit limbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mag {
    limbs: [u64; LIMBS],
}

impl Mag {
    const ZERO: Self = Self { limbs: [0; LIMBS] };

    pub(crate) fn from_u128(v: u128) -> Self {
        let mut m = Self::ZERO;
        let [lo, hi, ..] = &mut m.limbs;
        #[allow(clippy::cast_possible_truncation)] // the low and high halves
        {
            *lo = v as u64;
            *hi = (v >> 64) as u64;
        }
        m
    }

    /// The magnitude of an integral, finite, non-negative `f64`.
    fn from_f64(v: f64) -> Self {
        let bits = v.to_bits();
        let exp = i32::try_from((bits >> 52) & 0x7FF).unwrap_or(0);
        if exp < 1075 {
            // Below 2^53: exact in a `u64`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return Self::from_u128(u128::from(v as u64));
        }
        let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
        let shift = usize::try_from(exp - 1075).unwrap_or(0);
        let mut m = Self::ZERO;
        let (limb, bit) = (shift / 64, shift % 64);
        let wide = u128::from(mantissa) << bit;
        #[allow(clippy::cast_possible_truncation)] // the low and high halves
        for (k, part) in [wide as u64, (wide >> 64) as u64].into_iter().enumerate() {
            if let Some(slot) = m.limbs.get_mut(limb + k) {
                *slot = part;
            }
        }
        m
    }

    fn cmp(&self, b: &Self) -> Ordering {
        self.limbs.iter().rev().cmp(b.limbs.iter().rev())
    }

    fn add(&self, b: &Self) -> Self {
        let mut out = Self::ZERO;
        let mut carry = 0_u128;
        for ((o, x), y) in out.limbs.iter_mut().zip(&self.limbs).zip(&b.limbs) {
            let s = u128::from(*x) + u128::from(*y) + carry;
            #[allow(clippy::cast_possible_truncation)] // the low limb
            {
                *o = s as u64;
            }
            carry = s >> 64;
        }
        out
    }

    /// `self - b`, where `b` is not above `self`.
    fn sub(&self, b: &Self) -> Self {
        let mut out = Self::ZERO;
        let mut borrow = false;
        for ((o, x), y) in out.limbs.iter_mut().zip(&self.limbs).zip(&b.limbs) {
            let (d, b1) = x.overflowing_sub(*y);
            let (d, b2) = d.overflowing_sub(u64::from(borrow));
            *o = d;
            borrow = b1 || b2;
        }
        out
    }

    /// `self * b`; the limbs past [`LIMBS`] are dropped, which no product of
    /// two magnitudes of [`Int::abs_diff`] reaches.
    pub(crate) fn mul(&self, b: &Self) -> Self {
        let mut out = Self::ZERO;
        for (i, x) in self.limbs.iter().enumerate() {
            if *x == 0 {
                continue;
            }
            let mut carry = 0_u128;
            for (j, y) in b.limbs.iter().enumerate() {
                let Some(slot) = out.limbs.get_mut(i + j) else {
                    break;
                };
                let t = u128::from(*x) * u128::from(*y) + u128::from(*slot) + carry;
                #[allow(clippy::cast_possible_truncation)] // the low limb
                {
                    *slot = t as u64;
                }
                carry = t >> 64;
            }
        }
        out
    }

    /// Whether it is above `v`.
    pub(crate) fn above(&self, v: u64) -> bool {
        self.cmp(&Self::from_u128(u128::from(v))) == Ordering::Greater
    }

    /// Its decimal digits, as Python's `str` of an `int` writes them.
    pub(crate) fn decimal(&self) -> String {
        const CHUNK: u64 = 10_000_000_000_000_000_000; // 10^19
        let mut n = *self;
        // 19 digits a part: 34 parts hold every value of 34 limbs.
        let mut store = [0_u64; LIMBS];
        let mut count = 0;
        loop {
            // Divide by 10^19, from the top limb down.
            let mut rem = 0_u128;
            for limb in n.limbs.iter_mut().rev() {
                let cur = (rem << 64) | u128::from(*limb);
                #[allow(clippy::cast_possible_truncation)] // the quotient fits a limb
                {
                    *limb = (cur / u128::from(CHUNK)) as u64;
                }
                rem = cur % u128::from(CHUNK);
            }
            if let Some(slot) = store.get_mut(count) {
                #[allow(clippy::cast_possible_truncation)] // below 10^19
                {
                    *slot = rem as u64;
                }
                count += 1;
            }
            if n == Self::ZERO {
                break;
            }
        }
        let mut s = String::new();
        let parts = store.get(..count).unwrap_or_default();
        for (k, part) in parts.iter().rev().enumerate() {
            if k == 0 {
                let _ = write!(s, "{part}");
            } else {
                let _ = write!(s, "{part:019}");
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_are_exact() {
        let a = Int::of_f64(2f64.powi(121)).abs_diff(Int::I(0));
        let b = Int::I(1).abs_diff(Int::I(0));
        assert_eq!(a.mul(&b).decimal(), "2658455991569831745807614120560689152");
        let max = Int::of_f64(f64::MAX).abs_diff(Int::of_f64(-f64::MAX));
        let p = max.mul(&max);
        // (2 * f64::MAX)^2, as Python prints it: 618 digits.
        assert_eq!(p.decimal().len(), 618);
        assert!(p.decimal().starts_with("12926802428524400"));
        assert_eq!(Int::I(-5).abs_diff(Int::I(7)).decimal(), "12");
        assert_eq!(Mag::ZERO.decimal(), "0");
        assert!(Int::I(178_956_971).abs_diff(Int::I(0)).above(178_956_970));
    }

    #[test]
    fn ints_compare_and_subtract_as_python_s() {
        let big = Int::of_f64(1e38);
        assert_eq!(big, Int::F(1e38));
        assert_eq!(Int::of_f64(-3.0), Int::I(-3));
        assert_eq!(big.cmp(Int::I(i128::MAX)), Ordering::Greater);
        assert_eq!(Int::of_f64(-1e38).cmp(Int::I(i128::MIN)), Ordering::Less);
        assert_eq!(big.sub(big), Some(0));
        assert_eq!(big.sub(Int::I(0)), None);
        assert_eq!(Int::I(5).sub(Int::I(-2)), Some(7));
    }
}
