//! The dependency LRU and its pool.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{BudgetExceeded, Counting, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{CabResolver, DerefError, EnvOptions, Environment, LoadMode, PPtr, Source};
use unityrs_core::error::Error;
use unityrs_core::files::File;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

use crate::{
    TempFile, batch, ctx_with, key, keys, name_of, named_bundle, object, serialized_cab, src,
    unlimited,
};

/// `load_dependency` registers an evictable dependency whose meter names the
/// dependency pool and lists nothing in `files`; an evicted one opens again
/// on `Meter::reopen` (the same account, its work from zero, the old
/// count unchanged); a caller's file is never evicted; and every charge
/// returns.
#[test]
fn dependencies_are_evicted_and_reopened_and_every_charge_returns() {
    let budget = Arc::new(Counting::new(u64::MAX));
    let ctx = ctx_with(budget.clone());
    {
        let env = batch(ctx.clone(), 1);
        let caller = env
            .load(src("c.bundle", &named_bundle("CAB-C", 64)))
            .unwrap();
        let a = env
            .load_dependency(src("a.bundle", &named_bundle("CAB-A", 64)))
            .unwrap();
        assert_eq!(a.meter().reservation(), Reservation::Dependency);
        assert_eq!(env.files().count(), 1, "only the caller's file");
        let first = serialized_cab(&env, "cab-a");
        let old = first.meter().clone();
        let old_work = old.decoded();
        drop((a, first));
        env.load_dependency(src("b.bundle", &named_bundle("CAB-B", 64)))
            .unwrap();
        assert_eq!(env.stats().dependencies_evicted, 1);
        let again = serialized_cab(&env, "cab-a");
        assert!(!Arc::ptr_eq(again.meter(), &old));
        assert!(again.meter().same_input(&old));
        assert_eq!(old.decoded(), old_work, "the old count is unchanged");
        assert!(again.meter().decoded() <= old_work, "work from zero");
        assert_eq!(env.stats().dependencies_evicted, 2);
        let File::Bundle(cb) = &caller else { panic!() };
        let c1 = serialized_cab(&env, "cab-c");
        assert!(Arc::ptr_eq(
            &c1,
            &match cb.entry(0).unwrap() {
                File::Serialized(sf) => sf,
                _ => panic!(),
            }
        ));
    }
    assert_eq!(budget.used(), 0, "every charge returned");
}

/// `Source::Path` dependencies totalling twice the LRU's bound, read round
/// robin, are never refused by a pool sized for the bound and two more.
#[test]
fn path_dependencies_twice_the_bound_round_robin_are_never_refused() {
    let files: Vec<TempFile> = (0..6)
        .map(|i| TempFile::new("rr", &named_bundle(&format!("CAB-{i}"), 4096)))
        .collect();
    let one = {
        let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
        let f = env
            .load_dependency(Source::Path(files[0].0.clone()))
            .unwrap();
        let _ = serialized_cab(&env, "cab-0");
        f.meter().used()
    };
    let bound = 3 * one;
    let budget = Arc::new(Counting::new(bound + 3 * one));
    let env = batch(ctx_with(budget.clone()), bound);
    for f in &files {
        env.load_dependency(Source::Path(f.0.clone())).unwrap();
    }
    for _ in 0..4 {
        for i in 0..files.len() {
            let sf = serialized_cab(&env, &format!("cab-{i}"));
            assert_eq!(name_of(&object(&sf, 1)), format!("CAB-{i}"));
        }
    }
    assert!(env.stats().dependencies_evicted > 0);
}

/// A budget that grants inputs anything and dependencies up to a limit,
/// recording the most the dependencies held at once.
#[derive(Debug)]
struct DepPool {
    limit: AtomicU64,
    used: Arc<PoolHome>,
}

#[derive(Debug, Default)]
struct PoolHome {
    used: AtomicU64,
    peak: AtomicU64,
}

impl unityrs_core::budget::PermitHome for PoolHome {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl DepPool {
    fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            used: Arc::new(PoolHome::default()),
        }
    }
}

impl MemoryBudget for DepPool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Input {
            return Ok(Permit::detached(bytes));
        }
        let limit = self.limit.load(Ordering::Acquire);
        let used = &self.used.used;
        let now =
            used.try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(bytes).filter(|n| *n <= limit)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })? + bytes;
        self.used.peak.fetch_max(now, Ordering::AcqRel);
        Ok(Permit::new(bytes, self.used.clone()))
    }
}

/// A pool refusal evicts the cached dependencies and retries once; a
/// second refusal fails naming the dependency pool, and nothing is kept.
#[test]
fn a_pool_refusal_evicts_and_retries_once_then_fails_naming_the_pool() {
    let a = TempFile::new("pa", &named_bundle("CAB-A", 8192));
    let b = TempFile::new("pb", &named_bundle("CAB-B", 8192));
    let big = TempFile::new("pbig", &named_bundle("CAB-BIG", 64 << 10));
    // Measure one open: what it holds at most, and once loaded.
    let (peak, held) = {
        let pool = Arc::new(DepPool::new(u64::MAX));
        let env = batch(ctx_with(pool.clone()), u64::MAX);
        drop(env.load_dependency(Source::Path(a.0.clone())).unwrap());
        (
            pool.used.peak.load(Ordering::Acquire),
            pool.used.used.load(Ordering::Acquire),
        )
    };
    assert!(held > 0 && peak >= held);
    // Room for one open and less than the second's on top of the first.
    let pool = Arc::new(DepPool::new(peak + held / 2));
    let env = batch(ctx_with(pool.clone()), u64::MAX);
    drop(env.load_dependency(Source::Path(a.0.clone())).unwrap());
    drop(env.load_dependency(Source::Path(b.0.clone())).unwrap());
    assert_eq!(env.stats().dependencies_evicted, 1, "A made room for B");
    let refused = env
        .load_dependency(Source::Path(big.0.clone()))
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Budget(e) if e.what == Reservation::Dependency),
        "{refused:?}"
    );
    assert!(!keys(&env).contains(&"cab-big".to_owned()));
    assert!(env.files().next().is_none());
    assert_eq!(
        env.stats().dependency_bytes,
        0,
        "B was evicted for the retry"
    );
    drop(env);
    assert_eq!(pool.used.used.load(Ordering::Acquire), 0);
}

/// Under `unitypy()` every `find_file` load of a file that does not
/// register the name asked happens again at each dereference, as in
/// UnityPy; each replaces the last one's keys, and the LRU lets the
/// replaced one go (review round 1: the unbounded LRU kept every one, so
/// the memory grew with the number of dereferences).
#[test]
fn a_find_file_load_replaced_by_the_next_one_leaves_the_lru() {
    #[derive(Debug)]
    struct Elsewhere(Bytes);
    impl CabResolver for Elsewhere {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"cab-c").then(|| Source::Bytes {
                name: RawStr::from("@/cab-c"),
                data: self.0.clone(),
            })
        }
    }
    let target = bundle(&[
        ("CAB-OTHER", serialized(&[], &[Obj::named(5, "o5", &[])])),
        ("pad.resS", res_bytes(4096, 1)),
    ]);
    let a = bundle(&[(
        "CAB-A",
        serialized(&["archive:/CAB-C/CAB-C"], &[Obj::named(1, "a1", &[(1, 5)])]),
    )]);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::unitypy();
        o.load = mode;
        o.resolver = Some(Arc::new(Elsewhere(Bytes::from(target.clone()))));
        let env = Environment::new(o);
        env.load(src("a.bundle", &a)).unwrap();
        let sf = serialized_cab(&env, "cab-a");
        let mut held = Vec::new();
        for _ in 0..20 {
            let r = env.deref(
                &sf,
                PPtr {
                    file_id: 1,
                    path_id: 5,
                },
            );
            // The resolver's file is a bundle, the pointer's container.
            assert!(matches!(r, Err(DerefError::RawTarget { .. })), "{r:?}");
            held.push(env.stats().dependency_bytes);
        }
        assert_eq!(env.stats().dependencies_loaded, 20, "{mode:?}");
        // The first load's meter also holds its `files` entry, which the
        // later ones replace in place.
        assert!(held[1] > 0);
        assert_eq!(held[19], held[1], "{mode:?}: {held:?}");
    }
}

/// `unitypy()` options evict nothing, not even for a pool refusal (which
/// fails the read), so the paths the preload set stay (review round 1's
/// fix: a refusal evicted under every option).
#[test]
fn unitypy_options_evict_nothing_for_a_pool_refusal() {
    let a = named_bundle("CAB-A", 8192);
    let b = named_bundle("CAB-B", 8192);
    let (peak, held) = {
        let pool = Arc::new(DepPool::new(u64::MAX));
        let mut o = EnvOptions::unitypy();
        o.context = ctx_with(pool.clone());
        let env = Environment::new(o);
        drop(env.load_dependency(src("a.bundle", &a)).unwrap());
        (
            pool.used.peak.load(Ordering::Acquire),
            pool.used.used.load(Ordering::Acquire),
        )
    };
    assert!(held > 0 && peak >= held);
    // Room for one open and less than the second's on top of the first.
    let pool = Arc::new(DepPool::new(peak + held / 2));
    let mut o = EnvOptions::unitypy();
    o.context = ctx_with(pool.clone());
    let env = Environment::new(o);
    drop(env.load_dependency(src("a.bundle", &a)).unwrap());
    let refused = env.load_dependency(src("b.bundle", &b)).unwrap_err();
    assert!(
        matches!(&refused, Error::Budget(e) if e.what == Reservation::Dependency),
        "{refused:?}"
    );
    assert_eq!(env.stats().dependencies_evicted, 0);
    assert!(env.get_cab(&key("cab-a"), &unlimited()).unwrap().is_some());
}
