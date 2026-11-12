//! Test-only counts, built under this crate's `test-counters` feature (CR8's
//! counters' own switch), so a test can hold real work to the work counted:
//!
//! - the bytes every map of the cutter's tables (`ngui::table`'s `Map`)
//!   hashes, counted where they are hashed (its [`Counting`] hasher), and
//!   those the sprites kind's tag walk compares (B3's review rounds 4 and
//!   5, R1 and Q3);
//! - the late names the NGUI cutter's owner join looks at (round 5, Q1).
//!
//! The counts are the process's, so a test that reads one runs alone in its
//! binary.

use std::hash::{BuildHasher, DefaultHasher, Hasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};

static TAG_WALK: AtomicU64 = AtomicU64::new(0);
static LATE_SCANS: AtomicU64 = AtomicU64::new(0);

/// `bytes` bytes hashed or compared.
pub(crate) fn tag_walk(bytes: usize) {
    TAG_WALK.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// The bytes hashed (by any table's map) and compared (by the tag walk), in
/// this process.
#[must_use]
pub fn tag_walk_bytes() -> u64 {
    TAG_WALK.load(Ordering::Relaxed)
}

/// One late name looked at by the owner join.
pub(crate) fn late_scan() {
    LATE_SCANS.fetch_add(1, Ordering::Relaxed);
}

/// The late names the owner join has looked at, in this process.
#[must_use]
pub fn late_scans() -> u64 {
    LATE_SCANS.load(Ordering::Relaxed)
}

/// The standard library's hasher, every byte it is given counted
/// ([`tag_walk`]): the tables' maps' hasher under this feature.
#[derive(Clone, Debug, Default)]
pub struct Counting(RandomState);

impl BuildHasher for Counting {
    type Hasher = CountingHasher;

    fn build_hasher(&self) -> CountingHasher {
        CountingHasher(self.0.build_hasher())
    }
}

/// [`Counting`]'s hasher.
#[derive(Debug)]
pub struct CountingHasher(DefaultHasher);

impl Hasher for CountingHasher {
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    fn write(&mut self, bytes: &[u8]) {
        tag_walk(bytes.len());
        self.0.write(bytes);
    }
}
