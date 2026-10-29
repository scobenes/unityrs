//! Package C's files under a counting allocator (plan sections 3.15 and
//! 7.4): after every step of opening, loading and reading the testkit
//! matrix's bundles and files, and a bundle of many rows of one entry, the
//! heap the core holds is at most what the input's meter charged, plus the
//! meter's own constant; everything returns when the handles drop. Run under
//! both budgets. One test, so nothing else allocates while it runs.
//!
//!     scripts/dev cargo test -p unityrs-core --test files_memory

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::budget::{Counting as CountingBudget, MemoryBudget, Reservation, Unbounded};
use unityrs_core::detect::check_file_type;
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, build, matrix};
use unityrs_testkit::serialized::{build as file, one_object};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

// SAFETY: every method forwards to `System` and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(chunk(layout.size()), Ordering::Relaxed);
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(chunk(new_size), Ordering::Relaxed);
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live() -> u64 {
    LIVE.load(Ordering::Relaxed) as u64
}

/// The heap above `base` is within what `m` charged, plus the meter's own
/// allocations (`Meter::OWN_HEAP`, made after `base`).
/// `step` and `i` name the checkpoint; nothing is formatted unless it fails,
/// so the check allocates nothing itself.
fn within(base: u64, m: &Meter, name: &str, step: &str, i: usize) {
    let held = live().saturating_sub(base);
    assert!(
        held <= m.used() + Meter::OWN_HEAP,
        "{name} {step} {i}: heap {held} above the charge {} + {}",
        m.used(),
        Meter::OWN_HEAP
    );
}

fn load(name: &str, data: &Bytes, ctx: &Arc<FileContext>, limit: u64) {
    // The caller's name and handle are the caller's heap, made first. A
    // `Bytes` made from a `Vec` allocates its shared header (three words) on
    // its first clone; cloning both here makes that the caller's too, as it
    // is for a caller that keeps its own handle.
    let path = RawStr::from(name);
    let data = data.clone();
    let caller = (path.clone(), data.clone());
    let base = live();
    let m = Meter::new(limit, ctx.budget.clone(), Reservation::Input);
    let Ok(ty) = check_file_type(&data, &ctx.limits) else {
        return;
    };
    within(base, &m, name, "detect", 0);
    let parsed = parse_file(path, data, ty, None, &m, ctx);
    within(base, &m, name, "parse", 0);
    if let Ok(File::Bundle(b)) = &parsed {
        for i in 0..b.directory().len() {
            let _ = b.entry_bytes(i);
            within(base, &m, name, "bytes", i);
            let _ = b.entry(i);
            within(base, &m, name, "entry", i);
        }
        let got = b.load_all();
        within(base, &m, name, "load_all", 0);
        if let Err(unityrs_core::Error::Limit(l)) = &got {
            assert_eq!(l.kind, LimitKind::InputMemory, "{name}");
        }
    }
    drop(parsed);
    assert_eq!(m.used(), 0, "{name}: every charge returned");
    drop(m);
    assert_eq!(live(), base, "{name}: every allocation returned");
    drop(caller);
}

#[test]
fn files_hold_no_more_than_their_meter() {
    let mut inputs: Vec<(String, Bytes)> = matrix()
        .into_iter()
        .map(|(n, b)| (n, Bytes::from(b)))
        .collect();
    let one = file(&one_object(
        1,
        vec![NodeSpec::new(0, "GameObject", "Base")],
        vec![0; 4],
    ));
    let many = build(&BundleSpec {
        block_size: 1 << 17,
        entries: (0..1500)
            .map(|i| EntrySpec {
                name: format!("CAB-{i}").into_bytes(),
                data: one.clone(),
                flags: 4,
            })
            .collect(),
        ..BundleSpec::default()
    });
    inputs.push(("many-rows.bundle".into(), Bytes::from(many)));
    for counting in [false, true] {
        let budget: Arc<dyn MemoryBudget> = if counting {
            Arc::new(CountingBudget::new(u64::MAX))
        } else {
            Arc::new(Unbounded)
        };
        let mut ctx = FileContext::unitypy();
        ctx.budget = budget;
        let ctx = Arc::new(ctx);
        for (name, data) in &inputs {
            load(name, data, &ctx, u64::MAX);
            // And at a small limit, where the hostile one stops.
            load(name, data, &ctx, 1 << 20);
        }
    }
}
