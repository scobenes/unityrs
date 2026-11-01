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
//! The allocator counts only the measuring thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.
//! Two tests measure, and the counts are one set for the file, so each
//! measures on a thread of its own, one at a time ([`counting`]).

use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use unityrs_core::budget::{Counting, Reservation};
use unityrs_core::detect::check_file_type;
use unityrs_core::files::parse_file;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, RawStr};
use unityrs_dump::record::DirRow;
use unityrs_dump::{DumpOptions, Levels};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, build};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};
use unityrs_testkit::env::{Bundle, Obj, bundle, row, serialized};
use unityrs_testkit::serialized::{SerializedSpec, build as build_serialized};

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

/// Whether to refuse this fresh allocation: the size this thread armed,
/// once. It allocates nothing.
fn refuse(size: usize) -> bool {
    size != 0
        && REFUSE
            .try_with(|r| r.get() == size && r.replace(0) == size)
            .unwrap_or(false)
}

/// The test thread's live heap in chunks, its peak above the charge, and
/// the one fresh allocation it armed refused.
struct Watching;

impl Tally for Watching {
    fn refuses(&self, size: usize) -> bool {
        refuse(size)
    }

    /// A `realloc` is never refused: the core shrinks its tables into exact
    /// allocations, infallibly.
    fn refuses_realloc(&self, _new_size: usize) -> bool {
        false
    }

    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(chunk(size), Ordering::Relaxed);
        note_peak();
    }

    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

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

/// One measuring test at a time ([`counting`]).
static COUNTING: Mutex<()> = Mutex::new(());

/// `f` on a thread of its own whose allocations are counted, while no
/// other test measures: the thread has ended, its last frees counted,
/// before the next one starts. (A test's own libtest thread, counted, would
/// go on freeing after its test returned, into the next test's window.)
fn counting<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    let _one = COUNTING.lock().unwrap_or_else(PoisonError::into_inner);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
            f()
        })
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// The two sizes measured: what one row or object costs uncharged shows as
/// their difference times 19,000.
const SMALL: usize = 1_000;
const LARGE: usize = 20_000;

#[test]
fn the_dump_holds_nothing_uncharged_a_row_or_an_object() {
    counting(|| {
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
    });
}

/// A bundle whose engine string is `engine`, holding one SerializedFile with
/// no version string of its own, which takes the bundle's (the core's
/// version chain 2): two file records, each with the version packed.
fn engine_versioned(engine: &str) -> Vec<u8> {
    let mut entry = build_serialized(&SerializedSpec {
        unity_version: Vec::new(),
        ..SerializedSpec::default()
    });
    // Detection reads 128 bytes or more as a SerializedFile header.
    entry.resize(entry.len().max(128), 0);
    build(&BundleSpec {
        version_engine: engine.as_bytes().to_vec(),
        entries: vec![EntrySpec {
            name: b"CAB-versioned".to_vec(),
            data: entry,
            flags: 4,
        }],
        ..BundleSpec::default()
    })
}

/// The decimal digits `text` writes for `key`'s value.
fn digits_of(text: &str, key: &str) -> usize {
    text.split(&format!("\"{key}\":"))
        .nth(1)
        .map_or(0, |v| v.bytes().take_while(u8::is_ascii_digit).count())
}

/// Round 7's leftover (integration 7): a file record copied its packed
/// version, up to 226 limbs (1,808 bytes) past 2^128, uncharged, and
/// `json::write_packed` converted it to decimal in uncharged heap (a copy of
/// the limbs, a working copy, the base-10^19 digits and their string). The
/// record's copy is now charged to its `held` before it is made, and the
/// conversion works on the stack and writes into the record's charged line.
/// A bundle whose engine string is a 4,300-digit major of nines, with a
/// SerializedFile that takes that string, holds no more heap above the
/// charge, at its peak, than the same bundle at `2022.3.0f1`: both records
/// write the 4,315-digit value (`(10^4300 - 1) << 48 | 3 << 8 | 1`, its
/// digits counted by Python). On the old code this test measured 15,577
/// bytes above the charge against 64; with only the copies uncharged,
/// 1,801; with only the conversion's scratch on the heap, 13,769.
#[test]
fn a_big_packed_version_is_copied_charged_and_written_without_scratch() {
    let nines = format!("{}.0.0f1", "9".repeat(4300));
    let ((small, at_small), (big, at_big)) = counting(|| {
        (
            measured(&engine_versioned("2022.3.0f1")),
            measured(&engine_versioned(&nines)),
        )
    });
    whole(&small);
    whole(&big);
    for (key, text, digits) in [
        ("engine_version_packed", &small, 18),
        ("version_packed", &small, 18),
        ("engine_version_packed", &big, 4315),
        ("version_packed", &big, 4315),
    ] {
        assert_eq!(digits_of(text, key), digits, "{key}");
    }
    eprintln!(
        "heap above the charge at most {at_small} bytes at 2022.3.0f1, {at_big} at 4,300 nines"
    );
    // The same constant, give or take one allocation's rounding.
    assert!(
        at_big - at_small <= 64,
        "{at_small} bytes above the charge at 2022.3.0f1, {at_big} at a 4,300-digit major"
    );
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
