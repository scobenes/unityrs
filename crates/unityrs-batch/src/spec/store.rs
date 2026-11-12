//! Compact storage for the parts of a spec whose count no field caps:
//! `dependencies`, the `deps` lists, every bundle's `headers` and `prefer`
//! (`docs/plans/batch.md` 2.11 step 2, review round 1's D3).
//!
//! A 64 MiB document can hold some 20 million of them, so each is a few
//! words that point into shared tables: a [`Seg`] is a run of a table (a
//! string's bytes in the spec's string table, a bundle's headers in the
//! header table), and a [`Chunks`] grows by whole chunks, so it never
//! moves what it holds: no `realloc` copies a table, and none doubles its
//! peak while it grows (2.7's `realloc` rule).

use std::num::NonZeroU32;

/// A run of a table: `len` items from `start`. `Option<Seg>` is as small as
/// a `Seg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Seg {
    start: u32,
    /// `len + 1`, so `None` takes no room.
    len1: NonZeroU32,
}

/// The empty run at 0.
pub const EMPTY: Seg = Seg {
    start: 0,
    len1: NonZeroU32::MIN,
};

impl Seg {
    /// `len` items from `start`, or `None` past `u32`.
    #[must_use]
    pub fn new(start: usize, len: usize) -> Option<Self> {
        let start = u32::try_from(start).ok()?;
        let len1 = u32::try_from(len).ok()?.checked_add(1)?;
        Some(Self {
            start,
            len1: NonZeroU32::new(len1)?,
        })
    }

    /// The first item.
    #[must_use]
    pub fn start(self) -> usize {
        self.start as usize
    }

    /// How many items.
    #[must_use]
    pub fn len(self) -> usize {
        (self.len1.get() - 1) as usize
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Its items' range.
    #[must_use]
    pub fn range(self) -> std::ops::Range<usize> {
        self.start()..self.start() + self.len()
    }
}

/// An append-only list in chunks, each allocated at its final size: the
/// first holds [`FIRST`] items and each next twice its predecessor's, up
/// to [`LAST`], so a small list stays small and a large one never moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunks<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}

/// The indices `0..n` in bytewise order of `key(i)`, ties by index.
///
/// Each index is sorted as one integer, the key's first five bytes (zero
/// padded, big-endian) above the index's 24 bits, so the sort reads keys
/// only where those five bytes tie: an order of integers held in one array,
/// where comparing the keys themselves reads far apart (phase 1's time on
/// millions of ids, review round 2's P1). It takes 8 bytes an index while
/// it sorts. Past 2^24 indices, which no 64 MiB spec holds, the keys alone.
pub(crate) fn sort_by_bytes<'k>(n: u32, key: impl Fn(u32) -> &'k [u8]) -> Vec<u32> {
    const BITS: u32 = 24;
    const MASK: u64 = (1 << BITS) - 1;
    // The low bits are the index, below 2^24.
    #[allow(clippy::cast_possible_truncation)]
    let index = |k: u64| (k & MASK) as u32;
    if u64::from(n) > MASK {
        let mut order: Vec<u32> = (0..n).collect();
        order.sort_unstable_by(|&a, &b| key(a).cmp(key(b)).then(a.cmp(&b)));
        return order;
    }
    let mut keyed: Vec<u64> = (0..n)
        .map(|i| {
            let mut b = [0u8; 8];
            let k = key(i);
            for (to, from) in b.iter_mut().skip(3).zip(k) {
                *to = *from;
            }
            (u64::from_be_bytes(b) << BITS) | u64::from(i)
        })
        .collect();
    keyed.sort_unstable_by(|&x, &y| {
        (x >> BITS)
            .cmp(&(y >> BITS))
            .then_with(|| key(index(x)).cmp(key(index(y))))
            .then(x.cmp(&y))
    });
    keyed.into_iter().map(index).collect()
}

/// The first chunk's size.
const FIRST: usize = 16;
/// The largest chunk's size.
const LAST: usize = 1 << 14;
/// How many chunks grow before they reach [`LAST`].
const GROWING: u32 = (LAST / FIRST).ilog2();
/// The items the growing chunks hold together.
const GROWN: usize = FIRST * ((1 << GROWING) - 1);

/// Chunk `n`'s size: the items it holds when full, which [`locate`]
/// counts on. `Vec::with_capacity` may give more room than it is asked
/// for, so a chunk is full at this size, never at its capacity.
fn chunk_size(n: usize) -> usize {
    if n < GROWING as usize {
        FIRST << n
    } else {
        LAST
    }
}

/// The chunk that holds index `i`, and that chunk's first index.
fn locate(i: usize) -> (usize, usize) {
    if i < GROWN {
        let c = (i / FIRST + 1).ilog2();
        (c as usize, FIRST * ((1 << c) - 1))
    } else {
        let c = (i - GROWN) / LAST;
        (GROWING as usize + c, GROWN + c * LAST)
    }
}

impl<T> Default for Chunks<T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
}

impl<T> Chunks<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends `item`.
    pub fn push(&mut self, item: T) {
        let n = self.chunks.len();
        let full = self
            .chunks
            .last()
            .is_none_or(|c| c.len() == chunk_size(n - 1));
        if full {
            self.chunks.push(Vec::with_capacity(chunk_size(n)));
        }
        if let Some(c) = self.chunks.last_mut() {
            c.push(item);
            self.len += 1;
        }
    }

    /// The item at `i`.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<&T> {
        let (c, start) = locate(i);
        self.chunks.get(c)?.get(i - start)
    }

    /// Every item, in order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flatten()
    }

    /// The items of `seg`, in order.
    pub fn run(&self, seg: Seg) -> impl Iterator<Item = &T> {
        seg.range().map_while(|i| self.get(i))
    }

    /// Every item, mutably, in order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.chunks.iter_mut().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::{Chunks, LAST, Seg, chunk_size, sort_by_bytes};

    #[test]
    fn a_seg_is_a_range_and_its_option_is_small() {
        let s = Seg::new(3, 4).unwrap();
        assert_eq!(s.range(), 3..7);
        assert!(Seg::new(0, 0).unwrap().is_empty());
        assert_eq!(Seg::new(0, u32::MAX as usize), None);
        assert_eq!(size_of::<Option<Seg>>(), 8);
    }

    #[test]
    fn keys_sort_bytewise_ties_by_index() {
        let keys: [&[u8]; 9] = [
            b"abcdefg", b"abcde", b"abcdeff", b"", b"b", b"abcdefg", b"abcd", b"\xff", b"abcdea",
        ];
        let order = sort_by_bytes(9, |i| keys[i as usize]);
        let mut want: Vec<u32> = (0..9).collect();
        want.sort_by(|&a, &b| keys[a as usize].cmp(keys[b as usize]).then(a.cmp(&b)));
        assert_eq!(order, want);
    }

    #[test]
    fn chunks_index_across_their_growth() {
        let mut c = Chunks::new();
        // Past 60 chunks, where a shifted size would overflow.
        let n = 70 * LAST + 7;
        for i in 0..n {
            c.push(i);
        }
        assert_eq!(c.len(), n);
        assert!((0..n).all(|i| c.get(i) == Some(&i)));
        assert_eq!(c.get(n), None);
        assert!(c.iter().copied().eq(0..n));
        assert!(
            c.run(Seg::new(14, 4).unwrap())
                .copied()
                .eq([14, 15, 16, 17])
        );
        // Every chunk but the last holds its size, whatever its capacity.
        let full = c.chunks.len() - 1;
        assert!(
            c.chunks[..full]
                .iter()
                .enumerate()
                .all(|(n, k)| k.len() == chunk_size(n))
        );
        assert!(c.chunks.iter().all(|k| k.len() <= LAST));
    }
}
