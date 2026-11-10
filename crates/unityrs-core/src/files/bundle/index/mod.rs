//! The two tables a read searches (plan section 3.6, **Runs**): where each
//! block of the known prefix ends in the joined data ([`Ends`]), and the
//! cached runs by their first block ([`RunIndex`]). Each is charged to the
//! input's meter, and a search of either is a binary search, so a read
//! costs O(log B + log R) steps besides the blocks it decodes, where B is the
//! bundle's blocks and R its cached runs.
//!
//! **Steps are work.** Every input-driven step a read takes counts toward
//! the open's work (`Limits::max_decode_work`, `LimitKind::DecodeWork`), as
//! a decoded byte does, at [`STEP`] bytes a step: a search of `n` rows counts
//! [`search_steps`]`(n)`, and every block a read visits (learns, decodes into
//! a run or checks for a stored slice) and every run a publish drops counts
//! one. Tests see the steps through the open's meter (`Meter::decoded`).

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use bytes::Bytes;

use super::counts::RunBytes;
use crate::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};

/// The work one step counts: the bytes of one row a search reads (a block's
/// end offset, a run's first block), the unit the type index below format 16
/// counts per type (`files::serialized`).
pub(super) const STEP: u64 = 8;

/// The rows a binary search of `n` rows reads at most: ⌈log2 n⌉ + 1 for
/// `n > 0` (std's `partition_point` halves the rows ⌈log2 n⌉ times, then
/// reads the one left), and none of none.
pub(super) fn search_steps(n: usize) -> u64 {
    n.checked_sub(1).map_or(0, |m| u64::from(m.bit_width()) + 1)
}

/// Where each block of the known prefix ends in the joined data: the prefix
/// sums of the blocks' actual lengths (plan 3.6, "One layout"). Blocks
/// `0..known` all have known, non-error lengths, and `ends[k]` is the sum of
/// the lengths of blocks `0..=k`, so block `k` starts at `ends[k - 1]`. A
/// read learns the blocks after the prefix in order, and the prefix only
/// grows, so each block is learned once per open. The table has a row per
/// block, charged at open with the other per-block tables, and is read and
/// extended with no lock: a row is written before `known` passes it
/// (release, acquire), and two threads that learn one block write the same
/// value, since a block's length is published once
/// ([`crate::once::get_or_publish_result`]).
pub(crate) struct Ends {
    ends: Box<[AtomicU64]>,
    known: AtomicUsize,
}

impl Ends {
    /// The table for `blocks` blocks, none known, charged to `meter` (the
    /// caller keeps the charge with the bundle's).
    pub(crate) fn new(blocks: usize, meter: &Arc<Meter>) -> Result<(Self, Charge), ChargeError> {
        let mut rows = MeteredVec::with_capacity(meter, blocks)?;
        for _ in 0..blocks {
            rows.push(AtomicU64::new(0))?;
        }
        let (ends, charge) = rows.into_boxed_slice()?;
        let ends = Self {
            ends,
            known: AtomicUsize::new(0),
        };
        Ok((ends, charge))
    }

    /// How many blocks the known prefix holds.
    pub(super) fn known(&self) -> usize {
        self.known.load(Ordering::Acquire)
    }

    /// Where block `k` starts, for `k` at most [`Self::known`]: the end of
    /// block `k - 1`, or 0.
    pub(super) fn start(&self, k: usize) -> u64 {
        k.checked_sub(1).map_or(0, |j| self.end(j))
    }

    /// Where block `k` of the known prefix ends.
    pub(super) fn end(&self, k: usize) -> u64 {
        self.ends.get(k).map_or(0, |e| e.load(Ordering::Relaxed))
    }

    /// The first block of `from..known` that ends past `at`, or `known`
    /// (`known` as [`Self::known`] read it), and the rows the binary search
    /// read (its steps).
    pub(super) fn first_ending_past(&self, from: usize, known: usize, at: u64) -> (usize, u64) {
        self.search(from, known, |end| end <= at)
    }

    /// The first block of `from..known` that ends at or past `at`, or
    /// `known`, and the rows the binary search read.
    pub(super) fn first_ending_at(&self, from: usize, known: usize, at: u64) -> (usize, u64) {
        self.search(from, known, |end| end < at)
    }

    /// The first block of `from..known` whose end `before` rejects (ends
    /// rise with blocks, so `before` holds for a prefix of them), and the
    /// rows read: each probe counted, so a test sees the search's cost.
    fn search(&self, from: usize, known: usize, before: impl Fn(u64) -> bool) -> (usize, u64) {
        let rows = self.ends.get(from..known).unwrap_or_default();
        let mut probes = 0u64;
        let at = rows.partition_point(|e| {
            probes += 1;
            before(e.load(Ordering::Relaxed))
        });
        (from + at, probes)
    }

    /// Records that block `k`, whose length is `len`, ends where block `k`
    /// starts plus `len`, when every block before it is known (a read learns
    /// in order from the prefix's end, so it always is). Learning a block
    /// already known writes the value it holds.
    pub(super) fn learn(&self, k: usize, len: u64) {
        if k > self.known() {
            return;
        }
        let Some(row) = self.ends.get(k) else {
            return;
        };
        row.store(self.start(k).saturating_add(len), Ordering::Relaxed);
        self.known.fetch_max(k + 1, Ordering::Release);
    }
}

/// A cached run: blocks `first..end` decoded (or one slice of the input, for
/// stored blocks), keyed by `first` in the [`RunIndex`].
struct Run {
    end: usize,
    bytes: Bytes,
}

/// A node of the index's B-tree: its header, 11 keys and 11 runs; an
/// internal node adds 12 edges (std's `BTreeMap`, `B = 6`), as the
/// Environment's registry counts its own (`env::registry`).
const RUN_ELEMENT: usize = size_of::<usize>() + size_of::<Run>();
const RUN_LEAF: u64 = heap_cost((16 + 11 * RUN_ELEMENT) as u64);
const RUN_INTERNAL: u64 = heap_cost((16 + 11 * RUN_ELEMENT + 12 * size_of::<usize>()) as u64);

/// What one more run costs the index: a fifth of a leaf and a thirtieth of
/// an internal node. Every node of std's `BTreeMap` but the root holds at
/// least 5 of its 11 elements, and every internal node but the root at
/// least 6 children, so these shares cover the tree, with the root leaf
/// charged once, when the first run arrives (the registry's argument,
/// `env::registry`'s module docs; `tests/files_peak.rs` measures it).
pub(super) const RUN_SHARE: u64 = RUN_LEAF / 5 + 1 + RUN_INTERNAL / 30 + 1;

/// The cached runs, by first block. **No run contains another** (blocks
/// `a..b` contain `c..d` when `a <= c` and `d <= b`): a run is published
/// only when no cached run covers its span, and publishing it drops the
/// runs it covers. So as the first blocks rise the end blocks rise too, and
/// the run that starts last at or before a span's first block is the only
/// one that can cover the span: one search finds it. A publish drops the
/// runs it covers, which follow each other from its first block on. Every
/// span a run ever published covered stays covered by a cached one.
///
/// The tree is charged a share per run ([`RUN_SHARE`]) and its root leaf
/// once; each run's bytes carry their own charge (a decoded run's
/// `DecodeBuf`), or are a slice of the input (stored blocks).
pub(crate) struct RunIndex {
    runs: BTreeMap<usize, Run>,
    charge: Charge,
    /// Whether the root leaf is charged: the tree keeps it once allocated.
    rooted: bool,
    /// The runs' bytes, now and at most (`counts`, plan section 8's R3).
    bytes: RunBytes,
}

/// What a publish did.
pub(super) enum Published {
    /// The run is cached.
    Cached,
    /// A cached run covers the span: its first block and bytes.
    Covered(usize, Bytes),
    /// The meter refused the index's growth, or the room for the runs it
    /// would drop: nothing changed.
    Refused,
}

impl RunIndex {
    /// An empty index charging `meter`.
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            runs: BTreeMap::new(),
            charge: Charge::zero(meter),
            rooted: false,
            bytes: RunBytes::default(),
        }
    }

    /// How many runs are cached.
    pub(super) fn len(&self) -> usize {
        self.runs.len()
    }

    /// The runs' bytes, now and at most.
    pub(super) fn bytes(&self) -> &RunBytes {
        &self.bytes
    }

    /// The cached run that covers blocks `first..end`, if one does: its first
    /// block and bytes (type docs: the run starting last at or before
    /// `first`).
    pub(super) fn covering(&self, first: usize, end: usize) -> Option<(usize, Bytes)> {
        self.runs
            .range(..=first)
            .next_back()
            .filter(|(_, r)| r.end >= end)
            .map(|(f, r)| (*f, r.bytes.clone()))
    }

    /// The cached runs blocks `first..end` cover, by first block: they
    /// follow each other from `first` on (type docs).
    pub(super) fn inside(&self, first: usize, end: usize) -> impl Iterator<Item = usize> + '_ {
        self.runs
            .range(first..)
            .take_while(move |(_, r)| r.end <= end)
            .map(|(f, _)| *f)
    }

    /// How many cached runs blocks `first..end` cover.
    pub(super) fn covered(&self, first: usize, end: usize) -> usize {
        self.inside(first, end).count()
    }

    /// Publishes `bytes` as the run of blocks `first..end`: a cached run that
    /// covers it answers instead; otherwise the runs it covers move into
    /// `dropped` (to be released after the caller's lock) and it is cached.
    /// The room in `dropped` and the index's share are charged first, and a
    /// refusal changes nothing.
    pub(super) fn publish(
        &mut self,
        first: usize,
        end: usize,
        bytes: &Bytes,
        dropped: &mut MeteredVec<Bytes>,
    ) -> Published {
        if let Some((f, b)) = self.covering(first, end) {
            return Published::Covered(f, b);
        }
        let covered = self.covered(first, end);
        if covered > 0 && dropped.reserve_exact(covered).is_err() {
            return Published::Refused;
        }
        let share = if self.rooted {
            RUN_SHARE
        } else {
            RUN_SHARE + RUN_LEAF
        };
        if self.charge.grow(share).is_err() {
            return Published::Refused;
        }
        self.rooted = true;
        let mut gone = 0u64;
        for _ in 0..covered {
            let Some(f) = self.runs.range(first..).next().map(|(f, _)| *f) else {
                break;
            };
            if let Some(run) = self.runs.remove(&f) {
                gone = gone.saturating_add(len64(&run.bytes));
                // Within the room just reserved: no growth, no refusal.
                let _ = dropped.push(run.bytes);
            }
        }
        self.bytes.publish(len64(bytes), gone);
        self.runs.insert(
            first,
            Run {
                end,
                bytes: bytes.clone(),
            },
        );
        self.charge
            .release(RUN_SHARE.saturating_mul(u64::try_from(covered).unwrap_or(u64::MAX)));
        Published::Cached
    }
}

fn len64(bytes: &Bytes) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}
