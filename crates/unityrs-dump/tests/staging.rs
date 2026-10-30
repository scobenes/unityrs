//! What the dump holds while it writes an input's records, beside what the
//! core holds and the records themselves, is charged to the input's meter
//! before it is allocated (plan section 3.15; `unityrs_dump::input`,
//! "Memory"): the copies of the core's tables a file record's `l1` is built
//! from (block and directory rows, types, path ids, scripts, externals,
//! container rows), the record's path, the objects' order.
//!
//! A counting global allocator watches the test thread's heap through a
//! whole dump, in glibc chunks (`heap_cost`, the unit the meter charges), and
//! takes the peak of the heap above what the input's meter has charged.
//! Run on the dump before fix pass 4, this test measured 56 bytes a
//! directory row there, and 478 an object (with its type, its external and
//! its container row); now an input of 20,000 rows has the peak of one of
//! 1,000, so nothing a row costs goes uncharged. And a copy the allocator
//! refuses is
//! the input's `crash` record, never an abort of the run: the allocator
//! refuses one request of the exact size of the directory's copy.
//!
//! The allocator counts only the test's own thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use unityrs_core::budget::{Counting, Reservation};
use unityrs_core::detect::check_file_type;
use unityrs_core::files::parse_file;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, RawStr};
use unityrs_dump::record::DirRow;
use unityrs_dump::{DumpOptions, Levels};
use unityrs_testkit::counted::{count_this_thread, counted, others_are_not_counted};
use unityrs_testkit::env::{Bundle, Obj, bundle, row, serialized};

/// Live heap on the test thread, in glibc chunks.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The heap above the meter's charge at its peak, while measuring.
static PEAK_ABOVE: AtomicI64 = AtomicI64::new(i64::MIN);
/// The live heap and the budget's bookings when the measure began.
static BASE: AtomicI64 = AtomicI64::new(0);
/// The budget every charge of the measured dump is booked with.
static BUDGET: OnceLock<Arc<Counting>> = OnceLock::new();
std::thread_local! {
    /// A request size the allocator refuses on this thread, once (0: none).
    static REFUSE: Cell<usize> = const { Cell::new(0) };
}

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

fn signed(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The heap above the charge now: what the thread holds past `BASE`, less
/// what the budget has booked past it. It allocates nothing.
fn note_peak() {
    if let Some(budget) = BUDGET.get() {
        let live = signed(LIVE.load(Ordering::Relaxed) as u64);
        let above = live - signed(budget.used()) - BASE.load(Ordering::Relaxed);
        PEAK_ABOVE.fetch_max(above, Ordering::Relaxed);
    }
}

/// Whether to refuse this fresh allocation (a `realloc` is never refused:
/// the core shrinks its tables into exact allocations, infallibly): the size
/// this thread armed, once. It allocates nothing.
fn refuse(size: usize) -> bool {
    size != 0
        && REFUSE
            .try_with(|r| r.get() == size && r.replace(0) == size)
            .unwrap_or(false)
}

struct Watching;

// SAFETY: every method forwards to `System`, or refuses with null, and only
// counts, on the thread under test.
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && counted() {
            LIVE.fetch_add(chunk(layout.size()), Ordering::Relaxed);
            note_peak();
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: as above.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() && counted() {
            LIVE.fetch_add(chunk(layout.size()), Ordering::Relaxed);
            note_peak();
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counted() {
            LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        }
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let on = counted();
        // The worst case of a moving realloc: both blocks live at once.
        if on {
            LIVE.fetch_add(chunk(new_size), Ordering::Relaxed);
            note_peak();
        }
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if on {
            // A failed realloc leaves the old block live, and the new one
            // never was.
            let gone = if p.is_null() {
                chunk(new_size)
            } else {
                chunk(layout.size())
            };
            LIVE.fetch_sub(gone, Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOC: Watching = Watching;

/// A UnityFS bundle of `n` one-byte entries (`n` directory rows, a block
/// row per 160 bytes of data).
fn rows(n: usize) -> Vec<u8> {
    let names: Vec<String> = (0..n).map(|i| format!("e{i:06}")).collect();
    let entries: Vec<(&str, Vec<u8>)> = names.iter().map(|s| (s.as_str(), vec![7])).collect();
    bundle(&entries)
}

/// A SerializedFile of `n` named objects, each with its own type, `n`
/// externals and an AssetBundle of `n` container rows.
fn objects(n: usize) -> Vec<u8> {
    let externals: Vec<String> = (0..n).map(|i| format!("x{i:06}")).collect();
    let externals: Vec<&str> = externals.iter().map(String::as_str).collect();
    let paths: Vec<String> = (0..n).map(|i| format!("assets/a{i:06}")).collect();
    let n64 = i64::try_from(n).unwrap();
    let mut objs: Vec<Obj> = (1..=n64).map(|p| Obj::named(p, "o", &[(0, p)])).collect();
    objs.push(Obj::asset_bundle(
        n64 + 1,
        Bundle {
            rows: paths
                .iter()
                .enumerate()
                .map(|(i, p)| row(p, (0, 0), (0, i64::try_from(i).unwrap() + 1)))
                .collect(),
            ..Bundle::default()
        },
    ));
    serialized(&externals, &objs)
}

/// The dump of `data` on a meter booking with [`BUDGET`]: its text, and
/// the heap above the meter's charge at its peak.
fn measured(data: &[u8]) -> (String, i64) {
    let budget = BUDGET.get_or_init(|| Arc::new(Counting::new(u64::MAX)));
    let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let data = Bytes::copy_from_slice(data);
    let opts = DumpOptions {
        levels: Levels::ALL,
        ..DumpOptions::default()
    };
    BASE.store(
        signed(LIVE.load(Ordering::Relaxed) as u64) - signed(budget.used()),
        Ordering::Relaxed,
    );
    PEAK_ABOVE.store(0, Ordering::Relaxed);
    let d = unityrs_dump::input::dump_on(b"in", data, &meter, &opts);
    let peak = PEAK_ABOVE.swap(i64::MIN, Ordering::Relaxed);
    (d.text(), peak)
}

fn whole(text: &str) {
    assert!(
        !text.contains(r#""kind":"crash""#) && !text.contains(r#""kind":"parse""#),
        "{}",
        &text[..text.len().min(2000)]
    );
}

/// The two sizes measured: what one row or object costs uncharged shows as
/// their difference times 19,000.
const SMALL: usize = 1_000;
const LARGE: usize = 20_000;

#[test]
fn the_dump_holds_nothing_uncharged_a_row_or_an_object() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    for (what, input) in [
        ("directory rows", rows as fn(usize) -> Vec<u8>),
        ("objects", objects),
    ] {
        let (small, at_small) = measured(&input(SMALL));
        whole(&small);
        let (large, at_large) = measured(&input(LARGE));
        whole(&large);
        eprintln!(
            "{what}: heap above the charge at most {at_small} bytes at {SMALL}, {at_large} at {LARGE}"
        );
        // The same constant, give or take one allocation's rounding.
        assert!(
            at_large - at_small <= 64,
            "{what}: {at_small} bytes above the charge at {SMALL}, {at_large} at {LARGE}"
        );
    }
}

/// The copy of the directory the allocator refuses: the input's `crash`
/// record, the process alive to write it. Round 4: `memory allocation of
/// 5600000 bytes failed` and SIGABRT, for 100,000 rows.
#[test]
fn a_copy_the_allocator_refuses_is_the_inputs_crash() {
    const N: usize = 4_000;
    let data = rows(N);
    let opts = DumpOptions {
        levels: Levels::ALL,
        ..DumpOptions::default()
    };
    let whole_text = unityrs_dump::dump_input(b"in", Bytes::from(data.clone()), &opts).text();
    whole(&whole_text);
    let size = N * size_of::<DirRow>();
    // The core's load makes no fresh allocation of that size: the refusal
    // below is of the dump's own copy.
    REFUSE.with(|r| r.set(size));
    let ctx = &opts.context;
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let bytes = Bytes::from(data.clone());
    let ty = check_file_type(&bytes, &ctx.limits).unwrap();
    let file = parse_file(RawStr::from("in"), bytes, ty, None, &meter, ctx).unwrap();
    unityrs_dump::input::load_deep(&file).unwrap();
    assert_eq!(REFUSE.with(Cell::get), size, "the core's load asked for it");
    drop(file);
    let refused = unityrs_dump::dump_input(b"in", Bytes::from(data), &opts).text();
    assert_eq!(REFUSE.with(Cell::get), 0, "nothing asked for the copy");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert!(
        refused.contains(
            r#""error":{"kind":"crash","note":"unityrs ran out of memory dumping this input (memory budget refused "#
        ),
        "{refused}"
    );
}
