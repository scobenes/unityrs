//! The registry's memory model (plan sections 3.13 and 3.15), measured: the
//! keys an input names and the tables that hold them are charged to that
//! input's meter, so registering many names holds no more heap than the
//! meter charged, with the meter booking an `Unbounded` budget (UnityPy
//! mode) and a `Counting` one (a batch's pools, whose permits have a home).
//!
//! A counting global allocator watches the heap, in glibc chunks (the unit
//! the meter charges; `memory_model.rs` explains why), of the test's own
//! thread only (`unityrs_testkit::counted`): libtest's main thread allocates
//! beside it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::budget::{Counting as CountingBudget, MemoryBudget, Reservation, Unbounded};
use unityrs_core::env::{Cab, RawCab, Registry};
use unityrs_core::files::RawFile;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(chunk(size), Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
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
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    many_names(false);
    many_names(true);
}
