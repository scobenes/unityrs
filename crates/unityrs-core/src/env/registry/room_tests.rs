//! [`with_room`]'s rule where calls nest (plan section 3.13): a pool refusal
//! is retried by the innermost `with_room` it reaches, once, and by no other
//! (review round 8: once `resource` and `deref` took the retry, a lookup
//! inside a dereference inside a container pass retried one refusal at
//! every level, up to eight runs of the innermost work). Each closure counts
//! its runs, and its refusals come from a dependency meter whose budget
//! refuses, as a read's do.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use bytes::Bytes;

use super::{DepCache, open_bare, with_room};
use crate::budget::{BudgetExceeded, DecodeBuf, MemoryBudget, Permit, Reservation};
use crate::codec::{BlockCodec, Codec, Decoded, Stored, decode_block};
use crate::env::{Fetch, Source};
use crate::error::{Error, LimitKind};
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
/// itself, before [`super::from_callers`] counts it.
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

/// A fetcher's refusal of the pool is counted as it returns, whatever was
/// counted while the fetcher ran: its own once, and one the core made (and
/// counted where it made it) while the fetcher ran once more, since the
/// value returned cannot say which it is (`from_callers`; review round 9: a
/// fetcher's own refusal moved no count; review round 10: one counted only
/// when nothing had been was missed after a refusal the fetcher let go).
#[test]
fn a_fetchers_refusal_of_the_pool_is_counted_as_it_returns() {
    let ctx = Arc::new(FileContext::unitypy());
    let name = RawStr::from("f.bundle");
    for (fetch, what, counted) in [
        (RefusingFetch(None), "the fetcher's own", 1),
        (
            RefusingFetch(Some(refusing())),
            "the core's, during the fetch",
            2,
        ),
    ] {
        let source = Source::Fetch {
            name: name.clone(),
            fetch: Arc::new(fetch),
        };
        let at = pool_refusals();
        let r = open_bare(&name, &source, &refusing(), &ctx);
        assert!(matches!(r, Err(Error::Budget(_))), "{what}: {r:?}");
        assert_eq!(pool_refusals() - at, counted, "{what}");
    }
}

/// A fetcher that, on its first run, reads through the Environment (a
/// `with_room` of `cache`, whose retry meets the pool's refusal twice),
/// lets that refusal go and refuses the pool itself, and on its second run
/// serves no bytes (review round 10's probes).
#[derive(Debug)]
struct LetGoFetch {
    cache: Arc<DepCache>,
    meter: Arc<Meter>,
    runs: AtomicU32,
}

impl Fetch for LetGoFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        if self.runs.fetch_add(1, Ordering::AcqRel) > 0 {
            return Ok(Bytes::new());
        }
        let read = with_room(Some(&self.cache), None, || refused(&self.meter));
        assert!(read.is_err());
        Err(uncounted())
    }
}

/// The refusal a fetcher makes itself after it let go of one its own read
/// met twice is retried by the `with_room` around the fetch: the read's
/// mark does not hold it (review round 10: it was counted only when nothing
/// had been counted during the fetch, so it matched the mark and was passed
/// on untried).
#[test]
fn a_fetchers_own_refusal_after_one_it_let_go_is_retried() {
    let cache = Arc::new(cache());
    let fetch = Arc::new(LetGoFetch {
        cache: cache.clone(),
        meter: refusing(),
        runs: AtomicU32::new(0),
    });
    let name = RawStr::from("f.bundle");
    let source = Source::Fetch {
        name: name.clone(),
        fetch: fetch.clone(),
    };
    let ctx = Arc::new(FileContext::unitypy());
    let r = with_room(Some(&cache), None, || {
        open_bare(&name, &source, &Meter::unlimited(), &ctx).map(drop)
    });
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(fetch.runs.load(Ordering::Acquire), 2, "the fetcher's runs");
}

/// A fetcher that reads through the Environment (a `with_room` of `cache`,
/// whose retry, where it may take one, meets the pool's refusal again) and
/// passes that refusal on, counting its runs and its read's.
#[derive(Debug)]
struct PassesOn {
    cache: Arc<DepCache>,
    meter: Arc<Meter>,
    runs: AtomicU32,
    reads: AtomicU32,
}

impl Fetch for PassesOn {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.runs.fetch_add(1, Ordering::AcqRel);
        with_room(Some(&self.cache), None, || {
            self.reads.fetch_add(1, Ordering::AcqRel);
            refused(&self.meter)
        })
        .map(|()| Bytes::new())
    }
}

/// The price of counting every refusal a fetcher returns (`from_callers`):
/// one its own read already retried, passed on, cannot be told from the
/// fetcher's own, so the `with_room` around the fetch retries it once more.
/// That is one more fetch, whose read takes no retry of its own inside the
/// retry: two fetches and three reads, never more.
#[test]
fn a_refusal_a_fetcher_passes_on_costs_one_more_fetch() {
    let cache = Arc::new(cache());
    let fetch = Arc::new(PassesOn {
        cache: cache.clone(),
        meter: refusing(),
        runs: AtomicU32::new(0),
        reads: AtomicU32::new(0),
    });
    let name = RawStr::from("f.bundle");
    let source = Source::Fetch {
        name: name.clone(),
        fetch: fetch.clone(),
    };
    let ctx = Arc::new(FileContext::unitypy());
    let r = with_room(Some(&cache), None, || {
        open_bare(&name, &source, &Meter::unlimited(), &ctx).map(drop)
    });
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
    let runs = (
        fetch.runs.load(Ordering::Acquire),
        fetch.reads.load(Ordering::Acquire),
    );
    assert_eq!(runs, (2, 3), "(fetches, reads)");
}

/// A stored codec that refuses the pool itself on its first run.
struct RefusesOnce(AtomicU32);

impl BlockCodec for RefusesOnce {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        if self.0.fetch_add(1, Ordering::AcqRel) == 0 {
            return Err(uncounted());
        }
        Stored.decode(input, uncompressed, out)
    }
}

/// A codec's own refusal of the pool, after a refusal an inner `with_room`
/// met twice and let go in the same call, is retried: it is counted as it
/// returns (`from_callers`), so the inner one's mark does not hold it
/// (review round 10: a codec's refusal was never counted, so it matched
/// the mark and was passed on untried; a decryptor's likewise, which
/// `tests/env/callers.rs` pins through a load).
#[test]
fn a_codecs_own_refusal_after_one_let_go_is_retried() {
    let (cache, meter) = (cache(), refusing());
    let codec = RefusesOnce(AtomicU32::new(0));
    let input = Bytes::from_static(b"stored");
    let runs = Cell::new(0);
    let r = with_room(Some(&cache), None, || {
        runs.set(runs.get() + 1);
        let let_go = with_room(Some(&cache), None, || refused(&meter));
        assert!(let_go.is_err());
        let unlimited = Meter::unlimited();
        let mut out = DecodeBuf::new(&unlimited, 0, u64::MAX, LimitKind::InputMemory)?;
        decode_block(Codec::Other(5), &codec, &input, 6, &mut out, u64::MAX).map(drop)
    });
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(runs.get(), 2, "the call's runs");
}
