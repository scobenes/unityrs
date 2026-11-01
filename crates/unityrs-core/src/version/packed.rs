//! [`Packed`]: a packed version, an unbounded integer with Python's `int`
//! order, equality and hash.

use std::cmp::Ordering;
use std::fmt;

/// The packed integer, exactly. Most versions fit `u128`; a major of 2^80 or
/// more does not, and the canonical dump writes the exact integer. Ordered,
/// compared and hashed as Python's `int` (implemented by the scaffold).
///
/// The representation is private and always canonical: a value below 2^128
/// is held as a `u128`, and a larger one as little-endian limbs with a
/// non-zero top limb. So [`Packed::from_limbs`] of a value written with
/// leading zero limbs (a 4,300-digit group of leading zeros, sized from its
/// digit count) equals, hashes and orders as the same value from
/// [`Packed::from_u128`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Packed(Repr);

const _: () = crate::send_sync::<Packed>();

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Repr {
    /// Below 2^128.
    Small(u128),
    /// 2^128 or more: little-endian `u64` limbs, more than two, the last
    /// one non-zero.
    Big(Box<[u64]>),
}

impl Packed {
    /// The value `v`.
    #[must_use]
    pub const fn from_u128(v: u128) -> Self {
        Self(Repr::Small(v))
    }

    /// The value of little-endian `u64` limbs, any number of them, leading
    /// (most significant) zero limbs included.
    #[must_use]
    pub fn from_limbs(limbs: &[u64]) -> Self {
        let limbs = limbs.get(..significant(limbs)).unwrap_or_default();
        match limbs {
            [] => Self::from_u128(0),
            [lo] => Self::from_u128(u128::from(*lo)),
            [lo, hi] => Self::from_u128(u128::from(*lo) | (u128::from(*hi) << 64)),
            _ => Self(Repr::Big(limbs.into())),
        }
    }

    /// The value, when it is below 2^128.
    #[must_use]
    pub fn as_u128(&self) -> Option<u128> {
        match &self.0 {
            Repr::Small(v) => Some(*v),
            Repr::Big(_) => None,
        }
    }

    /// The limbs of a value of 2^128 or more, borrowed: little-endian, the
    /// last one non-zero, at most [`super::MAX_LIMBS`] for a version a
    /// parse packed. `None` below 2^128, where [`Self::as_u128`] is the
    /// value. Unlike [`Self::to_limbs`] it copies nothing, so a writer of an
    /// input's version needs no heap for it (`unityrs_dump::json::write_packed`).
    #[must_use]
    pub fn big_limbs(&self) -> Option<&[u64]> {
        match &self.0 {
            Repr::Small(_) => None,
            Repr::Big(limbs) => Some(limbs),
        }
    }

    /// The value as little-endian `u64` limbs, without leading zero limbs
    /// (empty for 0).
    #[must_use]
    pub fn to_limbs(&self) -> Vec<u64> {
        match &self.0 {
            Repr::Small(v) => {
                // Both halves of a u128, the high one first cut away when zero.
                let lo = u64::try_from(v & u128::from(u64::MAX)).unwrap_or(u64::MAX);
                let hi = u64::try_from(v >> 64).unwrap_or(u64::MAX);
                match (lo, hi) {
                    (0, 0) => Vec::new(),
                    (lo, 0) => vec![lo],
                    (lo, hi) => vec![lo, hi],
                }
            }
            Repr::Big(limbs) => limbs.to_vec(),
        }
    }

    /// `(self >> shift) & ((1 << width) - 1)`, for `width` up to 64.
    pub(super) fn bits(&self, shift: u32, width: u32) -> u64 {
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        match &self.0 {
            Repr::Small(v) => {
                u64::try_from(v.checked_shr(shift).unwrap_or(0) & u128::from(mask)).unwrap_or(0)
            }
            Repr::Big(limbs) => {
                let word = usize::try_from(shift / 64).unwrap_or(usize::MAX);
                let bit = shift % 64;
                let lo = limbs.get(word).copied().unwrap_or(0);
                let hi = limbs.get(word.saturating_add(1)).copied().unwrap_or(0);
                let v = (u128::from(hi) << 64 | u128::from(lo)) >> bit;
                u64::try_from(v & u128::from(mask)).unwrap_or(0)
            }
        }
    }

    /// The heap bytes the value owns (0 below 2^128).
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            Repr::Small(_) => 0,
            Repr::Big(limbs) => limbs.len() * 8,
        }
    }
}

/// The number of limbs up to and including the most significant non-zero
/// one: what [`Packed::from_limbs`] keeps.
pub(super) fn significant(limbs: &[u64]) -> usize {
    limbs.iter().rposition(|l| *l != 0).map_or(0, |i| i + 1)
}

impl Ord for Packed {
    fn cmp(&self, other: &Self) -> Ordering {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => a.cmp(b),
            // Canonical: every `Big` is at least 2^128, above every `Small`.
            (Repr::Small(_), Repr::Big(_)) => Ordering::Less,
            (Repr::Big(_), Repr::Small(_)) => Ordering::Greater,
            // Limbs are little-endian with a non-zero last limb: more limbs is
            // larger, else compare from the most significant limb down.
            (Repr::Big(a), Repr::Big(b)) => a
                .len()
                .cmp(&b.len())
                .then_with(|| a.iter().rev().cmp(b.iter().rev())),
        }
    }
}

impl PartialOrd for Packed {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Packed {
    /// The exact decimal integer. Past 2^128 it costs a pass over the
    /// limbs for each 19 digits, uncounted: the dump writes a packed
    /// version with its own conversion, which counts each pass as the
    /// input's work (`unityrs_dump::json::write_packed`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Repeated division by 10^19, least significant chunk first.
        const CHUNK: u64 = 10_000_000_000_000_000_000;
        let limbs = match &self.0 {
            Repr::Small(v) => return write!(f, "{v}"),
            Repr::Big(limbs) => limbs,
        };
        let mut n: Vec<u64> = limbs.to_vec();
        let mut chunks: Vec<u64> = Vec::new();
        while n.iter().any(|l| *l != 0) {
            let mut rem: u128 = 0;
            for l in n.iter_mut().rev() {
                let cur = (rem << 64) | u128::from(*l);
                *l = u64::try_from(cur / u128::from(CHUNK)).unwrap_or(u64::MAX);
                rem = cur % u128::from(CHUNK);
            }
            chunks.push(u64::try_from(rem).unwrap_or(0));
            while n.last() == Some(&0) {
                n.pop();
            }
        }
        let mut it = chunks.iter().rev();
        if let Some(top) = it.next() {
            write!(f, "{top}")?;
        }
        for c in it {
            write!(f, "{c:019}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Packed;

    #[test]
    fn a_value_has_one_form_whatever_its_leading_zeros() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let hash = |p: &Packed| {
            let mut h = DefaultHasher::new();
            p.hash(&mut h);
            h.finish()
        };
        // "000…01" sized from its 4,300 digits: 224 limbs, value 1.
        let mut limbs = vec![0_u64; 224];
        limbs[0] = 1;
        let padded = Packed::from_limbs(&limbs);
        let one = Packed::from_u128(1);
        assert_eq!(padded, one);
        assert_eq!(hash(&padded), hash(&one));
        assert_eq!(padded.cmp(&one), std::cmp::Ordering::Equal);
        assert!(padded < Packed::from_u128(2));
        // Two limbs fit u128; a zero top limb is dropped.
        assert_eq!(
            Packed::from_limbs(&[5, 7, 0]),
            Packed::from_u128(5 | (7 << 64))
        );
        assert_eq!(Packed::from_limbs(&[]), Packed::from_u128(0));
        assert_eq!(Packed::from_limbs(&[0, 0, 1]).to_limbs(), [0, 0, 1]);
        assert_eq!(
            Packed::from_u128(u128::MAX).to_limbs(),
            [u64::MAX, u64::MAX]
        );
        assert_eq!(Packed::from_u128(0).to_limbs(), Vec::<u64>::new());
        // Borrowed: a big value's own limbs, none below 2^128.
        let big = Packed::from_limbs(&[1, 2, 3, 0]);
        assert_eq!(big.big_limbs(), Some(&[1, 2, 3][..]));
        assert_eq!(Packed::from_u128(u128::MAX).big_limbs(), None);
    }

    #[test]
    fn big_packed_values_order_by_magnitude() {
        let small = Packed::from_u128(u128::MAX);
        let big_lo = Packed::from_limbs(&[u64::MAX, 0, 1]);
        let big_hi = Packed::from_limbs(&[0, 1, 1]);
        let bigger = Packed::from_limbs(&[0, 0, 0, 1]);
        assert!(small < big_lo);
        assert!(big_lo < big_hi, "compared from the most significant limb");
        assert!(big_hi < bigger);
        assert_eq!(big_lo.as_u128(), None);
    }
}
