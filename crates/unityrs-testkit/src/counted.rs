//! The thread a counting `#[global_allocator]` counts (plan section 7.3):
//! only the thread under test.
//!
//! libtest runs a test on a thread of its own while its main thread goes on
//! allocating (just after the spawn, and for its "has been running for over
//! 60 seconds" notice), so an allocator that counts every thread reads such
//! an allocation inside a measured window as the test's own, and a check
//! with no slack fails now and then. The core starts no thread of its own
//! ([`crate::waits`]), so the test's thread sees every allocation the core
//! makes for it. A test's first line is [`count_this_thread`], then
//! [`others_are_not_counted`] with the allocator's count.
//!
//! Owner: package B.

use std::cell::Cell;
use std::sync::Barrier;

std::thread_local! {
    /// Whether this thread's allocations are counted.
    static COUNTED: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread's allocations are counted. It allocates
/// nothing, so a global allocator may ask it.
#[must_use]
#[inline]
pub fn counted() -> bool {
    COUNTED.try_with(Cell::get).unwrap_or(false)
}

/// Counts the calling thread's allocations from now on.
pub fn count_this_thread() {
    COUNTED.with(|c| c.set(true));
}

/// Checks that an allocation on another thread does not move `live`, the
/// allocator's count: a second thread allocates 64 KiB and holds it while
/// the calling thread, which allocates nothing meanwhile, reads `live`
/// before and after. Run it before the test takes any baseline: the second
/// thread's start and end may leave a constant in the count (a block the
/// calling thread allocated for it and the other thread freed).
///
/// # Panics
///
/// When the other thread's allocation was counted.
pub fn others_are_not_counted(live: impl Fn() -> usize) {
    let barrier = Barrier::new(2);
    std::thread::scope(|s| {
        s.spawn(|| {
            barrier.wait();
            let block = std::hint::black_box(vec![1_u8; 1 << 16]);
            barrier.wait();
            barrier.wait();
            drop(block);
        });
        let before = live();
        barrier.wait();
        barrier.wait();
        let during = live();
        barrier.wait();
        assert_eq!(
            during, before,
            "another thread's allocation moved the count"
        );
    });
}
