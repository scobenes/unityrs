//! The heap of the crate's unit tests, for the tests that check a charge is
//! released after the memory it pays for (plan section 3.15): the global
//! allocator of the crate's unit tests is the testkit's counting one
//! (`unityrs_testkit::counted`), which counts only a thread that asks, and
//! a [`Noted`] budget records the counted heap at each release of its
//! permits. A test that drops a value whose charge is the last on a noted
//! meter to be released then compares the heap at that release with the
//! heap the drop leaves: equal, so nothing was freed after it.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

use crate::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use crate::limits::Meter;

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code (this file's `#[cfg(test)] mod heap;` is in
// another), so the item that names a thread carries one.
#[cfg(test)]
std::thread_local! {
    /// The heap of the thread, in bytes, once it counts: only the threads
    /// of the tests that call [`count`] ask to be, and each has its own
    /// count, so tests that count at once do not meet.
    static LIVE: Cell<usize> = const { Cell::new(0) };
}

/// [`LIVE`]'s tally. It allocates nothing.
struct Heap;

impl Tally for Heap {
    fn grew(&self, size: usize, _zeroed: bool) {
        let _ = LIVE.try_with(|l| l.set(l.get().wrapping_add(size)));
    }
    fn shrank(&self, size: usize) {
        let _ = LIVE.try_with(|l| l.set(l.get().wrapping_sub(size)));
    }
}

/// The global allocator of every unit test of the crate (module docs).
#[global_allocator]
static ALLOC: Allocator<Heap> = Allocator(Heap);

/// Counts the calling test's thread from now on, after checking that no
/// other thread is counted.
pub(crate) fn count() {
    count_this_thread();
    others_are_not_counted(live);
}

/// The calling thread's counted heap now.
pub(crate) fn live() -> usize {
    LIVE.try_with(Cell::get).unwrap_or(0)
}

/// A budget whose permits record, at each release, the releasing thread's
/// counted heap ([`live`]): the last release's, and how many there were,
/// so a release allocates nothing.
#[derive(Debug, Default)]
pub(crate) struct Noted {
    last: AtomicUsize,
    releases: AtomicUsize,
}

impl Noted {
    /// A meter of unbounded limit whose budget is a new [`Noted`].
    pub(crate) fn meter() -> (Arc<Meter>, Arc<Noted>) {
        let noted = Arc::new(Noted::default());
        let budget = Arc::new(Budget(noted.clone()));
        (Meter::new(u64::MAX, budget, Reservation::Input), noted)
    }

    /// The releases so far.
    pub(crate) fn releases(&self) -> usize {
        self.releases.load(Ordering::Relaxed)
    }
}

impl PermitHome for Noted {
    fn release(&self, _bytes: u64) {
        self.last.store(live(), Ordering::Relaxed);
        self.releases.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct Budget(Arc<Noted>);

impl MemoryBudget for Budget {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

/// Checks that `noted` saw a release after its first `from`, and that the
/// last saw the heap `after` (what the drop left): nothing was freed after
/// it.
pub(crate) fn released_last(noted: &Noted, from: usize, after: usize, what: &str) {
    assert!(noted.releases() > from, "{what}: no release");
    let at = noted.last.load(Ordering::Relaxed);
    assert_eq!(
        at,
        after,
        "{what}: {} bytes were freed after its release",
        at.wrapping_sub(after).cast_signed()
    );
}
