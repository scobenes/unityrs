//! What the Environment keeps, and what its evictions make, is paid for
//! (plan section 3.15), measured with a counting global allocator over the
//! heap of the test's own thread (`unityrs_testkit::counted`), in glibc
//! chunks (the unit the meter charges, `memory_model.rs`):
//!
//! - **A load's `files` entry** is charged to the loaded file's meter, a
//!   caller's load included: its share of the list and the indexes and its
//!   lowered key. The heap a caller's load keeps above what its meter
//!   charged is the same for a name of 64 bytes as for one of 64 KiB
//!   (review round 2: a caller's load charged nothing for its entry, and
//!   kept a lower-cased copy of its name built on an unlimited meter).
//! - **An eviction allocates nothing**: one over the bound, and a pool
//!   refusal's eviction of every other dependency, take one entry out at a
//!   time and drop it after the lock, so the heap a load that evicts peaks
//!   at is the same whether it evicts 3 dependencies or 999 (review round
//!   2: both collected what they evicted, and the refusal every key, in
//!   vectors no meter paid for).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use unityrs_core::budget::Reservation;
use unityrs_core::budget::{BudgetExceeded, Counting as CountingBudget, MemoryBudget, Permit};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::limits::heap_cost;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};
use unityrs_testkit::env::{Obj, res_bytes, serialized};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

fn grew(c: usize) {
    let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        grew(chunk(size));
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// A batch's pool whose limit the test sets after it loaded the
/// dependencies an eviction is to drop.
#[derive(Debug)]
struct Pool {
    limit: AtomicU64,
    inner: CountingBudget,
}

impl MemoryBudget for Pool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let limit = self.limit.load(Ordering::Relaxed);
        if self.inner.used().saturating_add(bytes) > limit {
            return Err(BudgetExceeded {
                requested: bytes,
                what,
            });
        }
        self.inner.try_reserve(bytes, what)
    }
}

/// A small dependency, a `SerializedFile` registered as `cab-{i}`.
fn small(i: usize) -> Source {
    Source::Bytes {
        name: RawStr::from(format!("CAB-{i}").as_str()),
        data: Bytes::from(serialized(&[], &[Obj::named(1, "s", &[])])),
    }
}

/// The tiny dependency a refused load retries: registered under `cab-0`, a
/// key the small ones made, so its registration adds no key whatever their
/// number.
fn tiny() -> Source {
    Source::Bytes {
        name: RawStr::from("CAB-0"),
        data: Bytes::from(serialized(&[], &[Obj::named(1, "t", &[])])),
    }
}

/// A file of the test's own, removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = self.0.parent().map(std::fs::remove_dir);
    }
}

/// A dependency whose meter alone passes `bound`: a raw file read from a
/// path (its bytes charged to its meter, in one buffer, and nothing parsed),
/// named `CAB-0`, a key the small ones made, so its registration adds no key
/// whatever their number.
fn over(bound: u64) -> TempFile {
    let dir = std::env::temp_dir().join(format!("unityrs-env-charges-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("CAB-0");
    std::fs::write(&path, res_bytes(usize::try_from(bound).unwrap() + 1, 5)).unwrap();
    TempFile(path)
}

/// The heap above its start that `load` peaks at.
fn peak_of(load: impl FnOnce()) -> usize {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    load();
    PEAK.load(Ordering::Relaxed) - base
}

/// The most small dependencies a test loads.
const MANY: usize = 1000;

/// `n` small dependencies under the bound, then one whose meter alone passes
/// it: its touch evicts the others. Returns the load's peak.
fn over_the_bound(n: usize) -> usize {
    let probe = Environment::new(EnvOptions::batch(Arc::new(FileContext::unitypy())));
    let one = probe.load_dependency(small(1)).unwrap().meter().used();
    drop(probe);
    let bound = one * (MANY as u64 + 1);
    let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    o.dep_cache_bytes = bound;
    let env = Environment::new(o);
    for i in 0..n {
        env.load_dependency(small(i)).unwrap();
    }
    assert_eq!(env.stats().dependencies_evicted, 0);
    let file = over(bound);
    let source = Source::Path(file.0.clone());
    let peak = peak_of(|| {
        let f = env.load_dependency(source).unwrap();
        assert!(f.meter().used() > bound, "it alone passes the bound");
    });
    // The one whose key it took is let go of, not evicted.
    assert_eq!(
        env.stats().dependencies_evicted,
        n as u64 - 1,
        "the others evicted"
    );
    peak
}

/// `n` small dependencies, then a pool that refuses the next load: its one
/// retry evicts all `n` first. Returns the load's peak.
fn refused_by_the_pool(n: usize) -> usize {
    let pool = Arc::new(Pool {
        limit: AtomicU64::new(u64::MAX),
        inner: CountingBudget::new(u64::MAX),
    });
    let mut ctx = FileContext::unitypy();
    ctx.budget = pool.clone();
    let env = Environment::new(EnvOptions::batch(Arc::new(ctx)));
    for i in 0..n {
        env.load_dependency(small(i)).unwrap();
    }
    pool.limit.store(pool.inner.used() + 64, Ordering::Relaxed);
    let source = tiny();
    let peak = peak_of(|| {
        env.load_dependency(source).unwrap();
    });
    // Refused before it registered anything: the retry's eviction takes all.
    assert_eq!(
        env.stats().dependencies_evicted,
        n as u64,
        "all {n} evicted"
    );
    peak
}

/// The heap one caller load of a file named `len` upper-case bytes keeps
/// above what the budget charged for it.
fn kept_above_charge(len: usize) -> i64 {
    let budget = Arc::new(CountingBudget::new(u64::MAX));
    let mut ctx = FileContext::unitypy();
    ctx.budget = budget.clone();
    let env = Environment::new(EnvOptions::batch(Arc::new(ctx)));
    let source = Source::Bytes {
        name: RawStr::from("A".repeat(len).as_str()),
        data: Bytes::from(serialized(&[], &[Obj::named(1, "a", &[])])),
    };
    let base = LIVE.load(Ordering::Relaxed);
    let file = env.load(source).unwrap();
    let kept = LIVE.load(Ordering::Relaxed) - base;
    let charged = budget.used();
    drop(file);
    i64::try_from(kept).unwrap() - i64::try_from(charged).unwrap()
}

#[test]
fn what_the_environment_keeps_or_evicts_is_paid_for() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let (short, long) = (kept_above_charge(64), kept_above_charge(64 << 10));
    assert_eq!(
        short, long,
        "a caller's load: the heap kept above its charge, for a 64-byte name, then a 64 KiB one"
    );
    let (few, many) = (over_the_bound(4), over_the_bound(MANY));
    assert_eq!(
        few, many,
        "over the bound: the load's peak with 4 dependencies to evict, then {MANY}"
    );
    let (few, many) = (refused_by_the_pool(4), refused_by_the_pool(MANY));
    assert_eq!(
        few, many,
        "a pool refusal: the load's peak with 4 dependencies to evict, then {MANY}"
    );
}
