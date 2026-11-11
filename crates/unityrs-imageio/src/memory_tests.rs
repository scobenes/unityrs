//! The crate's own measures under a counting allocator: the visible hash
//! allocates nothing (`crate::hash`, `docs/plans/extract.md` E59). They run
//! among the unit tests, beside what only a unit test reaches.
//!
//! This module installs the counting allocator for the crate's unit tests.
//! It counts only the thread under test (`unityrs_testkit::counted`), so
//! the other unit tests allocate unseen; the measures share its counts, so
//! one test runs them in turn.

use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

use crate::{Image, Mode, visible_hash};

/// Bytes live on the counted thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Blocks made on the counted thread, a `realloc`'s new block among them.
static MADE: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(size, Ordering::Relaxed);
        MADE.fetch_add(1, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// What `f` returns, and the blocks it made.
fn made_by<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = MADE.load(Ordering::Relaxed);
    let out = f();
    (out, MADE.load(Ordering::Relaxed) - before)
}

#[test]
fn the_measures_under_a_counting_allocator() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    the_hash_allocates_nothing();
}

fn the_hash_allocates_nothing() {
    let pixels: Vec<u8> = (0..4 * 3000_u32).map(|i| i.to_le_bytes()[0]).collect();
    // Every mode; one pixel, and images of several chunks of the hash's
    // stack buffer.
    for mode in Mode::ALL {
        for (width, height) in [(1, 1), (3000, 1), (1000, 3)] {
            let n = (width * height) as usize * mode.channels();
            let image = Image::new(width, height, mode, &pixels[..n]).unwrap();
            let (_, made) = made_by(|| visible_hash(&image));
            assert_eq!(made, 0, "{width}x{height} {mode}: blocks the hash made");
        }
    }
    // A side of `u32::MAX`, the widest the prefix writes.
    let widest = Image::new(u32::MAX, 0, Mode::Rgba, &[]).unwrap();
    let (_, made) = made_by(|| visible_hash(&widest));
    assert_eq!(made, 0, "4294967295x0: blocks the hash made");
}
