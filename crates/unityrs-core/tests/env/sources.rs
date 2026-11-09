//! Every [`Source`] is bounded by `Limits::max_input_bytes` before its
//! bytes are detected (`PARITY.md` 19.3, "a source's length"; review round
//! 10: a fetch was not bounded at all, against that row): a path from its
//! length (`env::tests`), a fetch from the size its fetcher states before
//! it is fetched and from the length of what it returns, given bytes from
//! their length. A refused fetch's bytes drop with their charge, and
//! nothing of a refused source is registered or listed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{Counting, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{EnvOptions, Environment, Fetch, LoadMode, Source};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::{Bytes, FileContext, RawStr};

use crate::named_bundle;

/// A fetcher of `data`, which states `hint` as its size and returns the
/// bytes charged to `pool` (as a fetcher that reserves them from the
/// dependency pool would), counting its runs.
#[derive(Debug)]
struct Charged {
    data: Vec<u8>,
    hint: Option<u64>,
    pool: Arc<Counting>,
    runs: AtomicU64,
}

/// Bytes that hold their permit, returned to the pool when they drop.
struct Owned(
    Vec<u8>,
    #[allow(dead_code, reason = "held for its drop")] Permit,
);

impl AsRef<[u8]> for Owned {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Fetch for Charged {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.runs.fetch_add(1, Ordering::AcqRel);
        let n = u64::try_from(self.data.len()).unwrap();
        let permit = self.pool.try_reserve(n, Reservation::Dependency)?;
        Ok(Bytes::from_owner(Owned(self.data.clone(), permit)))
    }

    fn size_hint(&self) -> Option<u64> {
        self.hint
    }
}

/// An Environment whose inputs may be `max` bytes long, under `mode`.
fn bounded(max: u64, mode: LoadMode) -> Environment {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_bytes = max;
    let mut o = EnvOptions::batch(Arc::new(ctx));
    o.load = mode;
    Environment::new(o)
}

/// What one load of a source gave: `Ok(())`, or the `InputBytes` refusal's
/// value; anything else fails the test.
fn outcome(r: Result<unityrs_core::files::File, Error>) -> Result<(), u64> {
    match r {
        Ok(_) => Ok(()),
        Err(Error::Limit(l)) if l.kind == LimitKind::InputBytes => Err(l.value),
        Err(e) => panic!("{e:?}"),
    }
}

/// One source of each kind, and how each kind is loaded, at a bound one
/// byte below the input and at its length: below, each is refused with its
/// length (a fetch whose fetcher states it, before it is fetched; one that
/// states nothing or too little, once fetched, its charge returned), and
/// nothing is registered, listed or kept; at the length, each loads.
#[test]
fn every_source_is_refused_above_its_bytes_bound_before_it_is_detected() {
    let data = named_bundle("CAB-A", 100);
    let len = u64::try_from(data.len()).unwrap();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for dependency in [false, true] {
            for (max, refused) in [(len - 1, true), (len, false)] {
                let what = format!("{mode:?} dependency {dependency} bound {max}");
                let env = bounded(max, mode);
                let load = |source: Source| {
                    outcome(if dependency {
                        env.load_dependency(source)
                    } else {
                        env.load(source)
                    })
                };
                let want = if refused { Err(len) } else { Ok(()) };
                let given = Source::Bytes {
                    name: RawStr::from("given.bundle"),
                    data: Bytes::from(data.clone()),
                };
                assert_eq!(load(given), want, "{what}: given bytes");
                // A fetch that states its size, one that states nothing, and
                // one that states too little: each runs only when its stated
                // size is within the bound, and is refused from what it
                // returned otherwise.
                for (hint, label) in [
                    (Some(len), "stated"),
                    (None, "unstated"),
                    (Some(1), "under"),
                ] {
                    let pool = Arc::new(Counting::new(u64::MAX));
                    let fetch = Arc::new(Charged {
                        data: data.clone(),
                        hint,
                        pool: pool.clone(),
                        runs: AtomicU64::new(0),
                    });
                    let source = Source::Fetch {
                        name: RawStr::from(format!("{label}.bundle").as_str()),
                        fetch: fetch.clone(),
                    };
                    let got = load(source);
                    let runs = fetch.runs.load(Ordering::Acquire);
                    assert_eq!(got, want, "{what}: a fetch, {label}");
                    let fetched = !(refused && hint == Some(len));
                    assert_eq!(runs, u64::from(fetched), "{what}: {label}'s fetches");
                    if refused {
                        assert_eq!(pool.used(), 0, "{what}: {label}'s bytes were kept");
                    }
                }
                if refused {
                    assert_eq!(
                        env.registry().len(),
                        0,
                        "{what}: a refused source registered"
                    );
                    assert_eq!(env.files().count(), 0, "{what}: a refused source listed");
                    assert_eq!(env.stats().dependencies_loaded, 0, "{what}");
                }
            }
        }
    }
}

/// A fetcher that serves `first` on its first fetch and `later` after.
#[derive(Debug)]
struct Grows {
    first: Bytes,
    later: Bytes,
    runs: AtomicU64,
}

impl Fetch for Grows {
    fn fetch(&self) -> Result<Bytes, Error> {
        let first = self.runs.fetch_add(1, Ordering::AcqRel) == 0;
        Ok(if first { &self.first } else { &self.later }.clone())
    }
}

/// A dependency reopened after an eviction goes through the same bound:
/// its fetcher, which served a bundle at the bound for the load, serves
/// one byte more for the reopen, which a lookup of its CAB then meets as
/// the `InputBytes` refusal.
#[test]
fn a_reopened_fetch_is_refused_above_its_bytes_bound() {
    let data = named_bundle("CAB-A", 100);
    let len = u64::try_from(data.len()).unwrap();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_input_bytes = len;
        let mut o = EnvOptions::batch(Arc::new(ctx));
        o.load = mode;
        // Room for one dependency at a time: each load evicts the other.
        o.dep_cache_bytes = 1;
        let env = Environment::new(o);
        let mut later = data.clone();
        later.push(0);
        let fetch = Arc::new(Grows {
            first: Bytes::from(data.clone()),
            later: Bytes::from(later),
            runs: AtomicU64::new(0),
        });
        let source = Source::Fetch {
            name: RawStr::from("a.bundle"),
            fetch: fetch.clone(),
        };
        drop(env.load_dependency(source).unwrap());
        let other = crate::src("b.bundle", &named_bundle("CAB-B", 100));
        drop(env.load_dependency(other).unwrap());
        assert!(env.stats().dependencies_evicted > 0, "{mode:?}");
        let got = env.get_cab(&RawStr::from("cab-a"), &crate::unlimited());
        assert!(
            matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputBytes && l.value == len + 1),
            "{mode:?}: {got:?}"
        );
        assert_eq!(fetch.runs.load(Ordering::Acquire), 2, "{mode:?}: fetches");
    }
}
