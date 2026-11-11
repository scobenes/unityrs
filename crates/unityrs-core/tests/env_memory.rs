//! The registry's memory model (plan sections 3.13 and 3.15), measured: the
//! keys an input names and the tables that hold them are charged to that
//! input's meter, so registering many names holds no more heap than the
//! meter charged, with the meter booking an `Unbounded` budget (UnityPy
//! mode) and a `Counting` one (a batch's pools, whose permits have a home).
//!
//! A counting global allocator watches the heap, in glibc chunks (the unit
//! the meter charges; `memory_model.rs` explains why), of the test's own
//! thread only (`unityrs_testkit::counted`): libtest's main thread allocates
//! beside it. The tests take [`SERIAL`] in turn, since each counts into the
//! same totals.
//!
//! A dependency's load under `LoadMode::Lazy` stages every registration
//! before it publishes the first (plan section 3.13, CR1): what the stage
//! holds is charged before it is allocated, so the heap above the charge,
//! at every allocation of the load, is the same for a bundle of many
//! entries as for one of few.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use unityrs_core::budget::{
    BudgetExceeded, Counting as CountingBudget, MemoryBudget, Permit, PermitHome, Reservation,
    Unbounded,
};
use unityrs_core::env::{Cab, EnvOptions, Environment, RawCab, Registry, Source};
use unityrs_core::files::RawFile;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};
use unityrs_testkit::env::{bundle, res_bytes};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// One test at a time counts.
static SERIAL: Mutex<()> = Mutex::new(());

/// What [`Watched`] has charged and not had returned.
static CHARGED: AtomicU64 = AtomicU64::new(0);
/// While set, each allocation records how far the heap above `HEAP_BASE`
/// passes the charge above `CHARGE_BASE`, the most of it in `OVER`.
static WATCHING: AtomicBool = AtomicBool::new(false);
static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
static CHARGE_BASE: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicI64 = AtomicI64::new(i64::MIN);

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        let live = LIVE.fetch_add(chunk(size), Ordering::Relaxed) + chunk(size);
        if WATCHING.load(Ordering::Relaxed) {
            let held = i64::try_from(live).unwrap_or(i64::MAX)
                - i64::try_from(HEAP_BASE.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
            let charged = i64::try_from(CHARGED.load(Ordering::Relaxed)).unwrap_or(i64::MAX)
                - i64::try_from(CHARGE_BASE.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
            OVER.fetch_max(held - charged, Ordering::Relaxed);
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

/// A batch's pool that books its charges in [`CHARGED`], which the
/// allocator reads.
#[derive(Debug)]
struct Watched(Arc<WatchedHome>);

#[derive(Debug)]
struct WatchedHome;

impl PermitHome for WatchedHome {
    fn release(&self, bytes: u64) {
        CHARGED.fetch_sub(bytes, Ordering::Relaxed);
    }
}

impl MemoryBudget for Watched {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        CHARGED.fetch_add(bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

fn live() -> u64 {
    u64::try_from(LIVE.load(Ordering::Relaxed)).unwrap()
}

/// Registers `n` names (mixed case, so each key is a lowered copy, and some
/// repeated, so a known key keeps its share) and checks after each that the
/// heap above the start is within what the meter charged; then that
/// dropping the registry returns it all.
fn many_names(counting: bool) {
    const N: usize = 3000;
    let budget: Arc<dyn MemoryBudget> = if counting {
        Arc::new(CountingBudget::new(u64::MAX))
    } else {
        Arc::new(Unbounded)
    };
    let meter = Meter::new(u64::MAX, budget, Reservation::Input);
    let names: Vec<RawStr> = (0..N)
        .map(|i| RawStr::from(format!("Dir/CAB-{:040}", i % (N - N / 10)).as_str()))
        .collect();
    let raw = RawFile::new(
        RawStr::from("CAB-RAW.resS"),
        Bytes::from_static(b"raw"),
        unityrs_core::detect::FileType::ResourceFile,
        &Meter::unlimited(),
    )
    .unwrap();
    let base = live();
    let used = meter.used();
    let registry = Registry::default();
    for (i, name) in names.iter().enumerate() {
        registry
            .register(name, Cab::Raw(RawCab::File(raw.clone())), &meter)
            .unwrap();
        if i % 97 == 0 || i + 1 == N {
            let held = live().saturating_sub(base);
            let charged = meter.used() - used;
            assert!(
                held <= charged,
                "counting {counting}, {} names: heap {held} above the charge {charged}",
                i + 1
            );
        }
    }
    assert_eq!(registry.len(), N - N / 10);
    let lookup = registry
        .get(
            &RawStr::from("DIR/cab-0000000000000000000000000000000000000005"),
            &meter,
        )
        .unwrap();
    assert!(lookup.is_some());
    drop(registry);
    assert_eq!(
        meter.used(),
        used,
        "counting {counting}: every charge returned"
    );
    assert!(live() <= base, "counting {counting}: the heap returned");
}

#[test]
fn many_registered_names_hold_no_more_heap_than_their_charge() {
    let _one = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    many_names(false);
    many_names(true);
}

/// The most the heap above its start passes what the pool charged, at any
/// allocation of the dependency load of a bundle of `n` entries with
/// mixed-case names (each key a lowered copy, staged with its own copy
/// while new), some repeated (each repeat staged as new, its copy and share
/// returned when the commit finds the key), loaded under `batch()`'s
/// options (`LoadMode::Lazy`).
fn staged_over(n: usize) -> i64 {
    let mut ctx = FileContext::unitypy();
    ctx.budget = Arc::new(Watched(Arc::new(WatchedHome)));
    let env = Environment::new(EnvOptions::batch(Arc::new(ctx)));
    let distinct = (n - n / 10).max(1);
    let names: Vec<String> = (0..n)
        .map(|i| format!("Dir/CAB-{:040}", i % distinct))
        .collect();
    let entries: Vec<(&str, Vec<u8>)> = names
        .iter()
        .map(|name| (name.as_str(), res_bytes(4, 1)))
        .collect();
    let source = Source::Bytes {
        name: RawStr::from("staged.bundle"),
        data: Bytes::from(bundle(&entries)),
    };
    HEAP_BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    CHARGE_BASE.store(CHARGED.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(i64::MIN, Ordering::Relaxed);
    WATCHING.store(true, Ordering::Relaxed);
    let file = env.load_dependency(source).unwrap();
    WATCHING.store(false, Ordering::Relaxed);
    assert_eq!(env.registry().len(), distinct, "{n} entries");
    drop(file);
    drop(env);
    OVER.load(Ordering::Relaxed)
}

/// CR1's stage is paid for: a lazy dependency's load of 2,000 entries holds
/// no more heap above its charge, at any allocation, than one of 20 (the
/// per-load constants: the meter, the dependency's record). Linux only, as
/// the other counting tests (CR1's review, round 1: no memory test loaded a
/// lazy bundle as a dependency).
#[test]
fn a_lazy_dependency_load_holds_no_more_heap_than_its_charge_at_any_allocation() {
    let _one = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let few = staged_over(20);
    let many = staged_over(2000);
    assert!(
        many <= few,
        "the heap passed the charge by {many} bytes at 2000 entries, by {few} at 20"
    );
}
