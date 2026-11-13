//! One coherent, allocation-free observation of a counted engine's heap
//! and its held charges (`docs/plans/extract.md` X12, X13): the test-only
//! stand-in for the engine's tally hook (`docs/plans/batch.md` 2.11's
//! `tally` row) until that hook lands, with the interface it is to keep, so
//! that the hook replaces this module's [`Ledger`] and no test changes.
//!
//! - **Registration** is [`crate::counted::count_this_thread`]: a test
//!   registers each engine thread before its measured work (Tokio's
//!   `on_thread_start` takes it as is); a thread never registered, the test
//!   runner's or a peer's, is never counted.
//! - **The heap** is the aggregate of every registered thread, one sum, so
//!   a block made on one thread and freed on another balances; no thread's
//!   signed count is read as what it owns.
//! - **Conservative**: a block counts from before `System` is asked for it
//!   until after `System` has freed it ([`crate::counted::Tally::growing`],
//!   [`crate::counted::Tally::shrank`]), so between two events the ledger
//!   never holds less than the registered threads' live heap, however
//!   their allocations interleave: what the lock orders is the count, and
//!   the count brackets each real transition.
//! - **Charges** are told to the same ledger ([`Ledger::charge`],
//!   [`Ledger::release`]): what a budget holds, never a reservation's
//!   unused part or a cap.
//!
//! Every event, a block or a charge, goes through one short critical
//! section, so each event sees the heap and the charges as one state, in
//! one total order: the residual it records, the heap's rise since
//! [`Ledger::arm`] less the charges' rise, is of one instant, never two
//! peaks read at different times. The critical section
//! is a spin on an atomic flag, which allocates nothing and cannot fail;
//! no event formats, allocates or panics, so a global allocator may call
//! it ([`crate::counted::Tally`]). Each event is an observation: a
//! residual past [`Ledger::arm`]'s uncharged allowance is a violation, the
//! first one kept with its event number.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use crate::counted::Tally;

/// No violation yet.
const NONE: u64 = u64::MAX;

/// The aggregate heap and charges of the registered threads, observed at
/// each event. `cost` is the charge unit of a block's size (the core's
/// `heap_cost`, which the testkit does not link): a test passes it.
pub struct Ledger {
    cost: fn(u64) -> u64,
    busy: AtomicBool,
    heap: AtomicI64,
    charged: AtomicI64,
    allowance: AtomicU64,
    events: AtomicU64,
    base: AtomicI64,
    base_charged: AtomicI64,
    peak: AtomicI64,
    max_residual: AtomicI64,
    max_at: AtomicU64,
    first_at: AtomicU64,
    first_heap: AtomicI64,
    first_charged: AtomicI64,
}

/// One event's view of the ledger, or a reading of it: one state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// Events since the last [`Ledger::arm`].
    pub events: u64,
    /// The registered threads' live heap, in the charge unit.
    pub heap: i64,
    /// The charges held.
    pub charged: i64,
    /// The most `heap` has been since the arm, above its value there.
    pub peak_above_arm: i64,
    /// The most any event since the arm saw of the residual: the heap's
    /// rise since the arm less the charges' rise (0 at the arm).
    pub max_residual: i64,
    /// The event that saw [`Self::max_residual`] (0: the arm itself).
    pub max_at: u64,
    /// The first event whose residual passed the allowance.
    pub first_violation: Option<Violation>,
}

/// An event whose heap passed its charges plus the allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The event's number since the arm.
    pub event: u64,
    /// The heap it saw.
    pub heap: i64,
    /// The charges it saw.
    pub charged: i64,
}

/// Holds the ledger's critical section; dropped, it lets the next event in.
struct Held<'a>(&'a AtomicBool);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn signed(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Ledger {
    /// An empty ledger whose blocks cost `cost(size)`.
    #[must_use]
    pub const fn new(cost: fn(u64) -> u64) -> Self {
        Self {
            cost,
            busy: AtomicBool::new(false),
            heap: AtomicI64::new(0),
            charged: AtomicI64::new(0),
            allowance: AtomicU64::new(0),
            events: AtomicU64::new(0),
            base: AtomicI64::new(0),
            base_charged: AtomicI64::new(0),
            peak: AtomicI64::new(0),
            max_residual: AtomicI64::new(0),
            max_at: AtomicU64::new(0),
            first_at: AtomicU64::new(NONE),
            first_heap: AtomicI64::new(0),
            first_charged: AtomicI64::new(0),
        }
    }

    fn hold(&self) -> Held<'_> {
        while self
            .busy
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        Held(&self.busy)
    }

    /// Starts a measured window: from now each event's residual, the heap's
    /// rise since now less the charges', is held against `allowance`, the
    /// uncharged bytes the window may hold, and the peaks and the first
    /// violation start again. The live heap and charges are not reset.
    pub fn arm(&self, allowance: u64) {
        let _held = self.hold();
        let heap = self.heap.load(Ordering::Relaxed);
        let charged = self.charged.load(Ordering::Relaxed);
        self.allowance.store(allowance, Ordering::Relaxed);
        self.events.store(0, Ordering::Relaxed);
        self.base.store(heap, Ordering::Relaxed);
        self.base_charged.store(charged, Ordering::Relaxed);
        self.peak.store(heap, Ordering::Relaxed);
        self.max_residual.store(0, Ordering::Relaxed);
        self.max_at.store(0, Ordering::Relaxed);
        self.first_at.store(NONE, Ordering::Relaxed);
    }

    /// One event: `heap` and `charged` move by the deltas, and the state
    /// after them is observed.
    fn event(&self, heap: i64, charged: i64) {
        let _held = self.hold();
        let h = self.heap.load(Ordering::Relaxed).saturating_add(heap);
        let c = self.charged.load(Ordering::Relaxed).saturating_add(charged);
        self.heap.store(h, Ordering::Relaxed);
        self.charged.store(c, Ordering::Relaxed);
        let n = self.events.load(Ordering::Relaxed).saturating_add(1);
        self.events.store(n, Ordering::Relaxed);
        if h > self.peak.load(Ordering::Relaxed) {
            self.peak.store(h, Ordering::Relaxed);
        }
        let residual = h
            .saturating_sub(self.base.load(Ordering::Relaxed))
            .saturating_sub(c.saturating_sub(self.base_charged.load(Ordering::Relaxed)));
        if residual > self.max_residual.load(Ordering::Relaxed) {
            self.max_residual.store(residual, Ordering::Relaxed);
            self.max_at.store(n, Ordering::Relaxed);
        }
        let allowance = signed(self.allowance.load(Ordering::Relaxed));
        if residual > allowance && self.first_at.load(Ordering::Relaxed) == NONE {
            self.first_at.store(n, Ordering::Relaxed);
            self.first_heap.store(h, Ordering::Relaxed);
            self.first_charged.store(c, Ordering::Relaxed);
        }
    }

    /// A budget granted `bytes`: held from this event on.
    pub fn charge(&self, bytes: u64) {
        self.event(0, signed(bytes));
    }

    /// A held charge of `bytes` was returned.
    pub fn release(&self, bytes: u64) {
        self.event(0, signed(bytes).saturating_neg());
    }

    /// The state now, read inside the critical section: one instant.
    #[must_use]
    pub fn observe(&self) -> Observation {
        let _held = self.hold();
        let first_at = self.first_at.load(Ordering::Relaxed);
        Observation {
            events: self.events.load(Ordering::Relaxed),
            heap: self.heap.load(Ordering::Relaxed),
            charged: self.charged.load(Ordering::Relaxed),
            peak_above_arm: self
                .peak
                .load(Ordering::Relaxed)
                .saturating_sub(self.base.load(Ordering::Relaxed)),
            max_residual: self.max_residual.load(Ordering::Relaxed),
            max_at: self.max_at.load(Ordering::Relaxed),
            first_violation: (first_at != NONE).then(|| Violation {
                event: first_at,
                heap: self.first_heap.load(Ordering::Relaxed),
                charged: self.first_charged.load(Ordering::Relaxed),
            }),
        }
    }
}

/// Counted conservatively: a block is counted before `System` is asked for
/// it and uncounted once `System` has freed it, so the ledger's heap is
/// never below the live heap, whatever the threads' interleaving; a
/// `realloc` counts its new block before the call and drops its old one
/// after it succeeds, and a failed request is rolled back.
impl Tally for Ledger {
    fn growing(&self, size: usize) {
        self.event(signed((self.cost)(size as u64)), 0);
    }

    fn failed(&self, size: usize) {
        self.event(signed((self.cost)(size as u64)).saturating_neg(), 0);
    }

    /// Counted already, by [`Self::growing`].
    fn grew(&self, _size: usize, _zeroed: bool) {}

    fn shrank(&self, size: usize) {
        self.event(signed((self.cost)(size as u64)).saturating_neg(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::{Ledger, Violation};
    use crate::counted::Tally;

    fn bytes(n: u64) -> u64 {
        n
    }

    #[test]
    fn a_charge_before_its_block_and_a_release_after_its_free_never_pass() {
        let l = Ledger::new(bytes);
        l.arm(0);
        l.charge(64);
        l.growing(64);
        l.shrank(64);
        l.release(64);
        let o = l.observe();
        assert_eq!(o.events, 4);
        assert_eq!((o.heap, o.charged), (0, 0));
        assert_eq!(o.peak_above_arm, 64);
        assert_eq!(o.max_residual, 0);
        assert_eq!(o.first_violation, None);
    }

    /// A block made before its charge is an uncharged instant: the first
    /// violation is kept, a later one does not replace it.
    #[test]
    fn an_allocation_before_its_reservation_is_the_first_violation() {
        let l = Ledger::new(bytes);
        l.arm(16);
        l.growing(64);
        l.charge(64);
        l.growing(32);
        let o = l.observe();
        assert_eq!(
            o.first_violation,
            Some(Violation {
                event: 1,
                heap: 64,
                charged: 0
            })
        );
        assert_eq!((o.max_residual, o.max_at), (64, 1));
        l.arm(64);
        assert_eq!(l.observe().first_violation, None, "the arm starts again");
        assert_eq!(l.observe().heap, 96, "the live heap is not reset");
    }

    #[test]
    fn the_cost_is_the_given_unit() {
        let l = Ledger::new(|n| n.div_ceil(16) * 16 + 16);
        l.growing(1);
        assert_eq!(l.observe().heap, 32);
        l.shrank(1);
        assert_eq!(l.observe().heap, 0);
    }
}
