//! What a bundle's block reads cost, counted as they happen (plan section
//! 8, R2 and R3): the bytes its decodes wrote, the bytes a decode wrote
//! only to have them dropped, the bytes it decoded again, and the bytes its
//! cached runs hold, now and at most. [`super::BundleFile::counts`] reads
//! them.
//!
//! **Cheap enough to leave on.** A decode adds to two relaxed atomics at
//! most, and a publish updates two integers under the run index's lock,
//! which it already holds: a count per block or per run, never per byte or
//! per object. A build with `--cfg unityrs_no_counters` compiles every
//! update out (the fields stay, so every charge is the same), which is how
//! the perf bench measures what they cost (plan section 10).
//!
//! Each count is a snapshot of relaxed atomics: a read on another thread
//! sees each one as some decode left it, not all of them at one instant.

use std::sync::atomic::{AtomicU64, Ordering};

/// What a bundle's block reads have done since it was opened (plan section
/// 8, R2 and R3). Its own blocks only: a nested bundle counts its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BundleCounts {
    /// Every byte a block decode wrote: first decodes, measuring decodes
    /// and decodes again, a stored block copied into a run included (the
    /// blocks-info section is not a block).
    pub decoded: u64,
    /// What a decode wrote that no run kept: a block measured before a read
    /// (it ends at or before the read's start, or every block is measured
    /// for the total) has its output dropped, and a run decoded while
    /// another thread published one covering it is dropped for that one.
    pub discarded: u64,
    /// What a decode wrote for a block an earlier decode of this open had
    /// already decoded: a block two spans share, decoded once for each, or a
    /// block measured earlier and decoded again into a read's run. A stored
    /// block whose length the open took from its stored bytes
    /// ([`crate::codec::Codecs::stores_as_is`]) is copied, not decoded, and
    /// never counts here. Two threads that decode one block at once, before
    /// either learns its length, each count a first decode.
    pub redecoded: u64,
    /// How many runs are cached now.
    pub runs: u64,
    /// The bytes the cached runs hold now: a decoded run's length, or a
    /// stored run's, which is a slice of the input.
    pub runs_held: u64,
    /// The most bytes the cached runs held at once since the open. A
    /// publish drops the runs the new one covers as it caches it, so the two
    /// never count together.
    pub runs_peak: u64,
}

const _: () = crate::send_sync::<BundleCounts>();

/// The decode counts, on any thread with no lock held.
#[derive(Debug, Default)]
pub(super) struct DecodeCounts {
    decoded: AtomicU64,
    discarded: AtomicU64,
    redecoded: AtomicU64,
}

impl DecodeCounts {
    /// A decode wrote `bytes`; `again` when its block was decoded before.
    pub(super) fn wrote(&self, bytes: u64, again: bool) {
        add(&self.decoded, bytes);
        if again {
            add(&self.redecoded, bytes);
        }
    }

    /// `bytes` a decode wrote were dropped unused.
    pub(super) fn dropped(&self, bytes: u64) {
        add(&self.discarded, bytes);
    }

    /// The counts, with the runs' (`held`).
    pub(super) fn snapshot(&self, runs: u64, held: &RunBytes) -> BundleCounts {
        BundleCounts {
            decoded: self.decoded.load(Ordering::Relaxed),
            discarded: self.discarded.load(Ordering::Relaxed),
            redecoded: self.redecoded.load(Ordering::Relaxed),
            runs,
            runs_held: held.held,
            runs_peak: held.peak,
        }
    }
}

/// The bytes the cached runs hold, and the most they held: kept under the
/// run index's lock.
#[derive(Debug, Default)]
pub(crate) struct RunBytes {
    held: u64,
    peak: u64,
}

impl RunBytes {
    /// A publish cached `added` bytes and dropped `dropped`.
    pub(crate) fn publish(&mut self, added: u64, dropped: u64) {
        if cfg!(not(unityrs_no_counters)) {
            self.held = self.held.saturating_add(added).saturating_sub(dropped);
            self.peak = self.peak.max(self.held);
        }
    }
}

/// Adds `n` to `counter` (relaxed): nothing reads a count to decide
/// anything.
fn add(counter: &AtomicU64, n: u64) {
    if cfg!(not(unityrs_no_counters)) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
}
