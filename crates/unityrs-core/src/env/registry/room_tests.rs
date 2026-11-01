//! [`with_room`]'s rule where calls nest (plan section 3.13): a pool refusal
//! is retried by the innermost `with_room` it reaches, once, and by no other
//! (review round 8: once `resource` and `deref` took the retry, a lookup
//! inside a dereference inside a container pass retried one refusal at
//! every level, up to eight runs of the innermost work). Each closure counts
//! its runs, and its refusals come from a dependency meter whose budget
//! refuses, as a read's do.

use std::cell::Cell;
use std::sync::Arc;

use super::{DepCache, with_room};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use crate::error::Error;
use crate::limits::{Limits, Meter};

/// A budget that refuses every reservation.
#[derive(Debug)]
struct Refuses;

impl MemoryBudget for Refuses {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        Err(BudgetExceeded {
            requested: bytes,
            what,
        })
    }
}

/// A dependency's meter whose every charge the pool refuses.
fn refusing() -> Arc<Meter> {
    Meter::for_input(
        &Limits::default(),
        Arc::new(Refuses),
        Reservation::Dependency,
    )
}

/// A read the pool refuses.
fn refused(meter: &Arc<Meter>) -> Result<(), Error> {
    meter.charge(8).map(drop).map_err(Error::from)
}

fn cache() -> DepCache {
    DepCache::new(u64::MAX, true)
}

/// A refusal an inner `with_room` retried and met again is the call's: the
/// one around it passes it on untried.
#[test]
fn a_refusal_an_inner_retry_met_is_passed_on() {
    let (cache, meter) = (cache(), refusing());
    let (outer, inner) = (Cell::new(0), Cell::new(0));
    let r = with_room(Some(&cache), None, || {
        outer.set(outer.get() + 1);
        with_room(Some(&cache), None, || {
            inner.set(inner.get() + 1);
            refused(&meter)
        })
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    assert_eq!((outer.get(), inner.get()), (1, 2), "(outer, inner) runs");
}

/// Inside a retry of an Environment's, no `with_room` of that Environment
/// retries: the room is made, and what the second run meets is final.
#[test]
fn nothing_retries_inside_a_retry() {
    let (cache, meter) = (cache(), refusing());
    let (outer, inner) = (Cell::new(0), Cell::new(0));
    let r = with_room(Some(&cache), None, || {
        outer.set(outer.get() + 1);
        if outer.get() == 1 {
            return refused(&meter);
        }
        with_room(Some(&cache), None, || {
            inner.set(inner.get() + 1);
            refused(&meter)
        })
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    assert_eq!((outer.get(), inner.get()), (2, 1), "(outer, inner) runs");
}

/// A refusal after one an inner `with_room` met and let go (a `settle`
/// leaves its refusal for the next call) is a new one, which the
/// `with_room` around it retries.
#[test]
fn a_refusal_after_one_let_go_is_retried() {
    let (cache, meter) = (cache(), refusing());
    let (outer, inner) = (Cell::new(0), Cell::new(0));
    let r = with_room(Some(&cache), None, || {
        outer.set(outer.get() + 1);
        let let_go = with_room(Some(&cache), None, || {
            inner.set(inner.get() + 1);
            refused(&meter)
        });
        assert!(let_go.is_err());
        refused(&meter)
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    assert_eq!(outer.get(), 2, "the outer's own refusal was not retried");
    // The inner one retried its own in the first run, and not in the
    // outer's retry.
    assert_eq!(inner.get(), 3, "inner runs");
}

/// Another Environment's `with_room` (a resolver or a fetcher that reads
/// another Environment from inside a retry) keeps its own retry.
#[test]
fn another_environments_retry_is_its_own() {
    let (a, b, meter) = (cache(), cache(), refusing());
    let (outer, inner) = (Cell::new(0), Cell::new(0));
    let r = with_room(Some(&a), None, || {
        outer.set(outer.get() + 1);
        if outer.get() == 1 {
            return refused(&meter);
        }
        with_room(Some(&b), None, || {
            inner.set(inner.get() + 1);
            refused(&meter)
        })
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    assert_eq!((outer.get(), inner.get()), (2, 2), "(outer, inner) runs");
}

/// A refusal that is not the pool's (another reservation's, a limit) is
/// never retried.
#[test]
fn only_the_pools_refusal_is_retried() {
    let cache = cache();
    let input = Meter::for_input(&Limits::default(), Arc::new(Refuses), Reservation::Input);
    let runs = Cell::new(0);
    let r = with_room(Some(&cache), None, || {
        runs.set(runs.get() + 1);
        refused(&input)
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    assert_eq!(runs.get(), 1);
}
