//! The partition cache is the only ASTC heap allocation. Check the heap at
//! each allocation, a host allocator refusal and a meter refusal.
use super::super::partition::PartitionTable;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};
use unityrs_testkit::counted::{self, Allocator, Tally};

#[cfg(test)]
std::thread_local! {
    static METER: RefCell<Option<Arc<Meter>>> = const { RefCell::new(None) };
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static EXCEEDED: Cell<bool> = const { Cell::new(false) };
    static REFUSE: Cell<usize> = const { Cell::new(0) };
    static REQUESTS: Cell<usize> = const { Cell::new(0) };
}

struct CacheTally;

impl Tally for CacheTally {
    fn refuses(&self, size: usize) -> bool {
        // Scope refusal to this thread; other tests can allocate freely.
        counted::counted() && REFUSE.with(|n| n.get() != 0 && n.get() == size)
    }

    fn grew(&self, size: usize, _: bool) {
        METER.with(|slot| {
            if let Some(meter) = slot.borrow().as_ref() {
                REQUESTS.with(|n| n.set(n.get() + 1));
                LIVE.with(|n| {
                    n.set(n.get() + size);
                    if u64::try_from(n.get()).unwrap_or(u64::MAX) > meter.used() {
                        EXCEEDED.with(|b| b.set(true));
                    }
                });
            }
        });
    }

    fn shrank(&self, size: usize) {
        METER.with(|slot| {
            if slot.borrow().is_some() {
                LIVE.with(|n| n.set(n.get().saturating_sub(size)));
            }
        });
    }
}

#[global_allocator]
static ALLOC: Allocator<CacheTally> = Allocator(CacheTally);

#[test]
fn cache_allocation_is_charged_and_fallible_from_the_first_request() {
    let need = PartitionTable::charge_bytes(12, 12).unwrap();
    counted::count_this_thread();
    for (limit, refuse, ok, requests) in [
        (need, 0, true, 1),
        (need, 442_368, false, 0),
        (need - 1, 0, false, 0),
    ] {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        METER.with(|slot| *slot.borrow_mut() = Some(meter.clone()));
        LIVE.with(|n| n.set(0));
        EXCEEDED.with(|n| n.set(false));
        REQUESTS.with(|n| n.set(0));
        REFUSE.with(|n| n.set(refuse));
        let got = PartitionTable::new(12, 12, &meter);
        REFUSE.with(|n| n.set(0));
        assert_eq!(got.is_ok(), ok);
        assert_eq!(REQUESTS.with(Cell::get), requests);
        assert!(!EXCEEDED.with(Cell::get));
        drop(got);
        assert_eq!(meter.used(), 0);
        assert_eq!(LIVE.with(Cell::get), 0);
        METER.with(|slot| *slot.borrow_mut() = None);
    }
}
