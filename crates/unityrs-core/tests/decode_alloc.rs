//! A decode buffer pays before it allocates (plan section 3.6): a growth the
//! budget refuses must not allocate first and give the memory back after.
//! A counting global allocator watches the heap; this file holds one test so
//! nothing else allocates while it runs.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::LimitKind;
use unityrs_core::budget::{Counting, DecodeBuf, Reservation};

struct Watching;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` and only counts sizes.
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let now = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        p
    }
}

#[global_allocator]
static ALLOC: Watching = Watching;

#[test]
fn a_refused_growth_allocates_nothing() {
    const BUDGET: u64 = 4096;
    const ASKED: usize = 64 << 20;
    let budget = Counting::new(BUDGET);
    let mut out = DecodeBuf::new(
        &budget,
        Reservation::BundleData,
        16,
        1 << 30,
        LimitKind::BlockOutput,
    )
    .unwrap();
    out.extend_from_slice(&[1; 16]).unwrap();

    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    // An LZMA block declared small that decodes on: the codec asks for 64 MiB.
    assert!(out.grow_zeroed(ASKED).is_err());
    let grew = PEAK.load(Ordering::Relaxed) - base;
    assert!(
        grew < 1 << 20,
        "a refused growth allocated {grew} bytes before the budget refused it"
    );

    // Growth the budget allows: the heap never holds more than it booked.
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let before = LIVE.load(Ordering::Relaxed);
    out.extend_from_slice(&[2; 2048]).unwrap();
    let grew = PEAK.load(Ordering::Relaxed) - before;
    assert!(budget.used() <= BUDGET);
    assert!(
        u64::try_from(grew).unwrap() <= BUDGET,
        "growth past the budget: {grew} bytes"
    );
    assert_eq!(out.len(), 16 + 2048);
}
