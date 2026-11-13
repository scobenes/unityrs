//! Count the runtime and every blocking-pool thread, excluding the peer.
use std::{
    cell::Cell,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};
use unityrs_batch::alloc::{Hook, Measured};
use unityrs_core::limits::heap_cost;
thread_local! { static NETWORK: Cell<bool> = const { Cell::new(false) }; }
thread_local! {
    static REFUSE: Cell<usize> = const { Cell::new(0) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
static SERIAL: Mutex<()> = Mutex::new(());
static PIECES: AtomicI64 = AtomicI64::new(0);
static LIBRARY_PEAK: AtomicI64 = AtomicI64::new(0);
pub struct Network;
fn active() -> bool {
    ACTIVE.load(Ordering::Acquire) && NETWORK.try_with(Cell::get).unwrap_or(false)
}
fn cost(size: usize) -> i64 {
    i64::try_from(heap_cost(u64::try_from(size).unwrap_or(u64::MAX))).unwrap_or(i64::MAX)
}
impl Hook for Network {
    fn refuses(&self, size: usize) -> bool {
        REFUSE
            .try_with(|r| {
                if size != 0 && r.get() == size {
                    r.set(0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
    fn grew(&self, size: usize, _: bool) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get().saturating_add(1)));
        if active() {
            let now = LIVE
                .fetch_add(cost(size), Ordering::AcqRel)
                .saturating_add(cost(size));
            PEAK.fetch_max(now, Ordering::AcqRel);
            LIBRARY_PEAK.fetch_max(now - PIECES.load(Ordering::Acquire), Ordering::AcqRel);
        }
    }
    fn shrank(&self, size: usize) {
        if active() {
            LIVE.fetch_sub(cost(size), Ordering::AcqRel);
        }
    }
}
/// Refuse the next allocation of this size on the calling thread; zero disarms.
#[allow(dead_code)]
pub fn refuse_next(size: usize) {
    REFUSE.with(|r| r.set(size));
}
#[allow(dead_code)]
pub fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}
pub fn lock() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub fn mark() {
    NETWORK.with(|v| v.set(true));
}
pub fn start() {
    mark();
    LIVE.store(0, Ordering::Release);
    PEAK.store(0, Ordering::Release);
    PIECES.store(0, Ordering::Release);
    LIBRARY_PEAK.store(0, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
}
pub fn finish() -> Measured {
    ACTIVE.store(false, Ordering::Release);
    Measured {
        kept: u64::try_from(LIVE.load(Ordering::Acquire)).unwrap_or(0),
        peak: u64::try_from(PEAK.load(Ordering::Acquire)).unwrap_or(0),
    }
}

/// Only exclude pieces already returned to and held by the caller. A newly
/// allocating piece still counts as library state until it returns, making
/// this stricter than subtracting every piece live at the allocation peak.
#[allow(dead_code)]
pub fn hold_pieces(bytes: u64) {
    PIECES.store(i64::try_from(bytes).unwrap(), Ordering::Release);
}
#[allow(dead_code)]
pub fn library_peak() -> u64 {
    u64::try_from(LIBRARY_PEAK.load(Ordering::Acquire)).unwrap_or(0)
}
