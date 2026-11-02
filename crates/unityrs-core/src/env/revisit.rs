//! What a container preload walk has yet to dereference
//! (`super::container_index`): every table index at first, and again an
//! index whose answer a load has since changed, the walk's own or another
//! thread's (review rounds 13 and 14).

use std::sync::Arc;

use crate::error::Error;
use crate::limits::{Meter, MeteredVec};

/// Bits a word of [`Pending`] holds.
const BITS: usize = u64::BITS as usize;

/// The most levels a [`Pending`] has: enough for `usize::MAX` indexes.
const MAX_LEVELS: usize = (usize::BITS as usize).div_ceil(6) + 1;

// 64^11 = 2^66 indexes: a level for each factor of 64 in `usize::MAX`.
const _: () = assert!(MAX_LEVELS >= 11);

/// The table indexes to dereference at the next row whose span reaches
/// them: a set of `0..len` as a tree of 64-bit words. Level 0's bit `i` is
/// index `i`; each level's bit `w` is set while the level below's word `w`
/// is not empty; the last level is one word. Finding the first index from
/// `i`, putting one back and taking one out each read at most two words a
/// level, and `len` indexes take `⌈log64 len⌉` levels, one at least (at
/// most [`MAX_LEVELS`]): a walk finds each index it dereferences at the
/// cost of a few words (plan decision 11), and can put an index back, which
/// the skip list it replaces could not. Its words are charged to the
/// walking file's meter.
pub(super) struct Pending {
    words: MeteredVec<u64>,
    /// Where each level's words start in `words`, and the end of the last.
    starts: [usize; MAX_LEVELS + 1],
    levels: usize,
}

impl Pending {
    /// Every index of `0..len`.
    pub(super) fn full(len: usize, meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut starts = [0; MAX_LEVELS + 1];
        let mut levels = 0;
        let mut total = 0_usize;
        let mut n = len.div_ceil(BITS).max(1);
        for end in starts.iter_mut().skip(1) {
            total = total.saturating_add(n);
            *end = total;
            levels += 1;
            if n == 1 {
                break;
            }
            n = n.div_ceil(BITS);
        }
        let mut words = MeteredVec::with_capacity(meter, total)?;
        // Level 0: `len` bits; each level above: a bit per word below,
        // every one of which holds a bit.
        let mut bits = len;
        for level in starts.windows(2).take(levels) {
            let &[start, end] = level else {
                break;
            };
            for w in 0..end - start {
                let left = bits.saturating_sub(w * BITS);
                words.push(if left >= BITS {
                    u64::MAX
                } else {
                    (1_u64 << left) - 1
                })?;
            }
            bits = end - start;
        }
        Ok(Self {
            words,
            starts,
            levels,
        })
    }

    /// Its levels: the words each of its operations reads, at most.
    pub(super) fn levels(&self) -> usize {
        self.levels
    }

    /// Where word `w` of `level` is in `words`, if the level has it.
    fn at(&self, level: usize, w: usize) -> Option<usize> {
        let start = *self.starts.get(level)?;
        let end = *self.starts.get(level.checked_add(1)?)?;
        start.checked_add(w).filter(|&at| at < end)
    }

    fn word(&self, level: usize, w: usize) -> Option<u64> {
        let at = self.at(level, w)?;
        self.words.as_slice().get(at).copied()
    }

    fn word_mut(&mut self, level: usize, w: usize) -> Option<&mut u64> {
        let at = self.at(level, w)?;
        self.words.as_mut_slice().get_mut(at)
    }

    /// The first index from `from` in the set.
    pub(super) fn first_from(&self, from: usize) -> Option<usize> {
        let mut level = 0;
        let mut at = from;
        loop {
            let word = self.word(level, at / BITS)?;
            let rest = word & (u64::MAX << (at % BITS));
            if rest != 0 {
                let mut found = (at / BITS) * BITS + rest.trailing_zeros() as usize;
                while level > 0 {
                    level -= 1;
                    let below = self.word(level, found)?;
                    found = found * BITS + below.trailing_zeros() as usize;
                }
                return Some(found);
            }
            level += 1;
            if level == self.levels {
                return None;
            }
            at = at / BITS + 1;
        }
    }

    /// Takes `i` out.
    pub(super) fn remove(&mut self, i: usize) {
        let mut at = i;
        for level in 0..self.levels {
            let Some(word) = self.word_mut(level, at / BITS) else {
                return;
            };
            *word &= !(1 << (at % BITS));
            if *word != 0 {
                return;
            }
            at /= BITS;
        }
    }

    /// Puts `i` back (an index of `0..len`).
    pub(super) fn insert(&mut self, i: usize) {
        let mut at = i;
        for level in 0..self.levels {
            let Some(word) = self.word_mut(level, at / BITS) else {
                return;
            };
            let was = *word;
            *word |= 1 << (at % BITS);
            if was != 0 {
                return;
            }
            at /= BITS;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::Pending;
    use crate::limits::Meter;
    use unityrs_testkit::rng::Rng;

    fn all(p: &Pending) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = p.first_from(0);
        while let Some(i) = at {
            out.push(i);
            at = p.first_from(i + 1);
        }
        out
    }

    /// The set is a `BTreeSet` of `0..len` under every operation, on sizes
    /// that fill a level, pass it by one, and need four levels.
    #[test]
    fn pending_is_an_ordered_set() {
        let meter = Meter::unlimited();
        let mut rng = Rng::new(0x0013_d0d0);
        for len in [1, 2, 63, 64, 65, 4095, 4096, 4097, 300_000] {
            let mut p = Pending::full(len, &meter).unwrap();
            let mut model: BTreeSet<usize> = (0..len).collect();
            // One level per factor of 64, and one word at the top.
            let levels = match len {
                0..=64 => 1,
                65..=4096 => 2,
                4097..=262_144 => 3,
                _ => 4,
            };
            assert_eq!(p.levels(), levels, "{len}");
            assert_eq!(p.first_from(len), None, "{len}");
            assert_eq!(p.first_from(len + 1000), None, "{len}");
            for _ in 0..4000 {
                let i = rng.below(len);
                match rng.below(3) {
                    0 => {
                        p.remove(i);
                        model.remove(&i);
                    }
                    1 => {
                        p.insert(i);
                        model.insert(i);
                    }
                    _ => {}
                }
                let from = rng.below(len + 2);
                assert_eq!(
                    p.first_from(from),
                    model.range(from..).next().copied(),
                    "{len} from {from}"
                );
            }
            if len <= 4097 {
                assert_eq!(all(&p), model.iter().copied().collect::<Vec<_>>());
                for i in 0..len {
                    p.remove(i);
                }
                assert_eq!(p.first_from(0), None, "{len}: emptied");
                p.insert(len - 1);
                assert_eq!(p.first_from(0), Some(len - 1), "{len}");
            }
        }
    }

    /// Its words are charged to the meter before they are made.
    #[test]
    fn pending_charges_its_words() {
        let meter = Meter::unlimited();
        let p = Pending::full(4097, &meter).unwrap();
        // 65 words at level 0, 2 at level 1, 1 at level 2.
        assert_eq!(p.words.len(), 68);
        assert!(meter.used() >= 68 * 8);
    }
}
