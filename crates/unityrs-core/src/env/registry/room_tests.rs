//! [`with_room`]'s rule where calls nest (plan section 3.13): a pool refusal
//! is retried by the innermost `with_room` it reaches, once, and by no other
//! (review round 8: once `resource` and `deref` took the retry, a lookup
//! inside a dereference inside a container pass retried one refusal at
//! every level, up to eight runs of the innermost work). Each closure counts
//! its runs, and its refusals come from a dependency meter whose budget
//! refuses, as a read's do.

use std::cell::Cell;
use std::sync::Arc;

use bytes::Bytes;

use super::{DepCache, open_source, with_room};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use crate::env::{Fetch, Source};
use crate::error::Error;
use crate::limits::{Limits, Meter, pool_refusals};
use crate::options::FileContext;
use crate::raw::RawStr;

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

/// A refusal of the pool that moves no count: one the caller's code made
/// itself, before [`super::dependency`]'s `fetched` counts it.
fn uncounted() -> Error {
    Error::Budget(BudgetExceeded {
        requested: 8,
        what: Reservation::Dependency,
    })
}

/// A call whose closure runs once refused by `refusal`, then succeeds: its
/// result and how many times it ran.
fn refused_once(cache: &DepCache, refusal: impl Fn() -> Error) -> (Result<(), Error>, u32) {
    let runs = Cell::new(0);
    let r = with_room(Some(cache), None, || {
        runs.set(runs.get() + 1);
        if runs.get() == 1 {
            return Err(refusal());
        }
        Ok(())
    });
    (r, runs.get())
}

/// Whether a refusal is retried depends on the current call alone: the
/// mark an earlier call left, its refusal retried and met again, ends with
/// that call, so a later call's refusal is retried even when no count moved
/// between them (review round 9: the mark stayed, and the later refusal, a
/// fetcher's own, matched it and was passed on untried).
#[test]
fn a_mark_an_earlier_call_left_holds_no_later_refusal() {
    let (cache, meter) = (cache(), refusing());
    let earlier = with_room(Some(&cache), None, || refused(&meter));
    assert!(matches!(earlier, Err(Error::Budget(_))), "{earlier:?}");
    let (r, runs) = refused_once(&cache, uncounted);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(runs, 2, "the later call's refusal was not retried");
}

/// An outermost call that ends in a panic, a refusal an inner one retried
/// marked first, leaves no mark either.
#[test]
fn a_call_that_panics_leaves_no_mark() {
    let (cache, meter) = (cache(), refusing());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Error> = with_room(Some(&cache), None, || {
            let inner = with_room(Some(&cache), None, || refused(&meter));
            assert!(inner.is_err());
            panic!("the call panics with the mark set");
        });
    }));
    assert!(panicked.is_err());
    let (r, runs) = refused_once(&cache, uncounted);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(runs, 2, "a mark outlived the call that panicked");
}

/// A fetcher that refuses the pool: by its own refusal, or by one the core
/// made (and counted) on `meter` while it ran.
#[derive(Debug)]
struct RefusingFetch(Option<Arc<Meter>>);

impl Fetch for RefusingFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        match &self.0 {
            Some(meter) => refused(meter).map(|()| Bytes::new()),
            None => Err(uncounted()),
        }
    }
}

/// A fetcher's own refusal of the pool is counted as one the core made,
/// once, and one the core made and counted while the fetcher ran is not
/// counted again (review round 9: a fetcher's refusal moved no count).
#[test]
fn a_fetchers_refusal_of_the_pool_is_counted_once() {
    let ctx = Arc::new(FileContext::unitypy());
    let name = RawStr::from("f.bundle");
    for (fetch, what) in [
        (RefusingFetch(None), "the fetcher's own"),
        (
            RefusingFetch(Some(refusing())),
            "the core's, during the fetch",
        ),
    ] {
        let source = Source::Fetch {
            name: name.clone(),
            fetch: Arc::new(fetch),
        };
        let at = pool_refusals();
        let r = open_source(&name, &source, &refusing(), &ctx);
        assert!(matches!(r, Err(Error::Budget(_))), "{what}: {r:?}");
        assert_eq!(pool_refusals() - at, 1, "{what}");
    }
}
