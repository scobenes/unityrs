//! Package C's files at their peak (plan section 3.15): while a bundle opens,
//! a `SerializedFile` parses and a lazy read publishes a run over many
//! cached ones, the heap the test thread holds never rises above what the
//! input's meter has charged, at any allocation, not only between steps.
//! A scratch buffer the core allocates without charging it (a stable sort's
//! scratch, a list of the runs a new one covers) shows here as soon as it
//! is allocated, and it grows with the input, so each case runs at two
//! sizes a hundred times apart.
//!
//! The allocator counts the test thread's allocations only, so libtest's
//! own threads (its output capture, the other tests) never reach the count,
//! and every case runs with nothing of its own allocated before the meter's
//! first charge but what the case made beforehand on purpose.
//!
//!     scripts/dev cargo test -p unityrs-core --test files_peak

#![allow(unsafe_code)]

use std::cell::Cell;
use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::detect::check_file_type;
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread};
use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec, build as file};

struct Counting;

thread_local! {
    /// Whether this thread's allocations are counted.
    static WATCHED: Cell<bool> = const { Cell::new(false) };
    /// The heap this thread allocated since the watch began, in glibc
    /// chunks (`heap_cost`, the meter's unit).
    static LIVE: Cell<i64> = const { Cell::new(0) };
    /// The meter the watch compares with, and what it had charged when the
    /// watch began.
    static METER: Cell<*const Meter> = const { Cell::new(std::ptr::null()) };
    static USED0: Cell<i64> = const { Cell::new(0) };
    /// The most the heap rose above the meter's charge, at any allocation.
    static PEAK: Cell<i64> = const { Cell::new(i64::MIN) };
}

fn chunk(n: usize) -> i64 {
    i64::try_from(heap_cost(n as u64)).unwrap_or(i64::MAX)
}

/// Counts `delta` bytes on this thread when it is watched; on a growth,
/// compares the heap with what the meter has charged. Allocates nothing.
fn note(delta: i64) {
    let _ = WATCHED.try_with(|w| {
        if !w.get() {
            return;
        }
        let live = LIVE.get() + delta;
        LIVE.set(live);
        let meter = METER.get();
        if delta > 0 && !meter.is_null() {
            // SAFETY: `watch` holds the meter's `Arc` for as long as the
            // pointer is set.
            let used = i64::try_from(unsafe { (*meter).used() }).unwrap_or(i64::MAX);
            PEAK.set(PEAK.get().max(live - (used - USED0.get())));
        }
    });
}

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        note(chunk(size));
    }
    fn shrank(&self, size: usize) {
        note(-chunk(size));
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// Runs `f` with this thread watched against `meter`, and returns its value
/// and the most the heap it allocated rose above what the meter charged
/// while it ran.
fn watch<T>(meter: &Arc<Meter>, f: impl FnOnce() -> T) -> (T, i64) {
    count_this_thread();
    USED0.set(i64::try_from(meter.used()).unwrap_or(i64::MAX));
    LIVE.set(0);
    PEAK.set(i64::MIN);
    METER.set(Arc::as_ptr(meter));
    WATCHED.set(true);
    let out = f();
    WATCHED.set(false);
    METER.set(std::ptr::null());
    (out, PEAK.get())
}

/// A fresh input meter, with no budget.
fn meter() -> Arc<Meter> {
    Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input)
}

/// The caller's input: its bytes, already shared (a `Bytes` made from a
/// `Vec` allocates its shared header on the first clone, which is the
/// caller's), and the detected type.
fn input(data: Vec<u8>) -> (Bytes, unityrs_core::detect::FileType, Arc<FileContext>) {
    let ctx = Arc::new(FileContext::unitypy());
    let data = Bytes::from(data);
    drop(data.clone());
    let ty = check_file_type(&data, &ctx.limits).unwrap();
    (data, ty, ctx)
}

/// A bundle of `rows` directory rows with distinct paths, no bytes each.
fn many_rows(rows: usize) -> Vec<u8> {
    build(&BundleSpec {
        info_codec: TestCodec::None,
        entries: (0..rows)
            .map(|i| EntrySpec {
                name: format!("e{:06}", rows - i).into_bytes(),
                data: Vec::new(),
                flags: 0,
            })
            .collect(),
        ..BundleSpec::default()
    })
}

/// The peak of opening a bundle (its headers, blocks-info and directory,
/// and UnityPy's `files` dict over the directory).
fn open_peak(rows: usize) -> i64 {
    let (data, ty, ctx) = input(many_rows(rows));
    let m = meter();
    let name = RawStr::from("rows");
    let (file, peak) = watch(&m, || parse_file(name, data, ty, None, &m, &ctx));
    let Ok(File::Bundle(b)) = file else {
        panic!("{rows} rows: not a bundle");
    };
    assert_eq!(b.directory().len(), rows);
    peak
}

/// A `SerializedFile` of `objects` objects of one type, path ids
/// descending, so the `objects` dict's sort moves every row.
fn many_objects(objects: usize) -> Vec<u8> {
    file(&SerializedSpec {
        types: vec![TypeSpec {
            class_id: 1,
            nodes: vec![NodeSpec::new(0, "GameObject", "Base")],
            ..TypeSpec::default()
        }],
        objects: (0..objects)
            .map(|i| ObjectSpec {
                path_id: i64::try_from(objects - i).unwrap(),
                type_id: 0,
                class_id: 1,
                data: Vec::new(),
                ..ObjectSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    })
}

/// The peak of parsing a `SerializedFile` (its tables and the `objects`
/// dict's order).
fn parse_peak(objects: usize) -> i64 {
    let (data, ty, ctx) = input(many_objects(objects));
    let m = meter();
    let name = RawStr::from("CAB-objects");
    let (file, peak) = watch(&m, || parse_file(name, data, ty, None, &m, &ctx));
    let Ok(File::Serialized(s)) = file else {
        panic!("{objects} objects: not a SerializedFile");
    };
    assert_eq!(s.objects().count(), objects);
    peak
}

/// A bundle of `runs` 16-byte LZ4 blocks, one entry per block, and one more
/// entry patched to span them all (blocks-info stored, so its row is in the
/// file as written).
fn covered_runs(runs: usize) -> Vec<u8> {
    let mut entries: Vec<EntrySpec> = (0..runs)
        .map(|i| EntrySpec {
            name: format!("r{i:06}").into_bytes(),
            data: vec![u8::try_from(i % 251).unwrap(); 16],
            flags: 0,
        })
        .collect();
    entries.push(EntrySpec {
        name: b"cover".to_vec(),
        data: Vec::new(),
        flags: 0,
    });
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 16,
        entries,
        ..BundleSpec::default()
    });
    let key = b"cover\0";
    let at = data.windows(key.len()).position(|w| w == key).unwrap();
    let size = i64::try_from(runs * 16).unwrap();
    data[at - 20..at - 12].copy_from_slice(&0i64.to_be_bytes());
    data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    data
}

/// The peak of a lazy read whose run covers `runs` cached runs, one per
/// block, each read before it: publishing it drops them all.
fn cover_peak(runs: usize) -> i64 {
    let (data, ty, ctx) = input(covered_runs(runs));
    let m = meter();
    let Ok(File::Bundle(b)) = parse_file(RawStr::from("runs"), data, ty, None, &m, &ctx) else {
        panic!("{runs} runs: not a bundle");
    };
    let mut small = Vec::with_capacity(runs);
    for i in 0..runs {
        small.push(b.entry_bytes(i).unwrap());
    }
    // The small runs stay cached; the reads' own slices go first.
    drop(small);
    let (cover, peak) = watch(&m, || b.entry_bytes(runs));
    let cover = cover.unwrap();
    assert_eq!(cover.len(), runs * 16);
    drop(cover);
    peak
}

/// UnityPy's `files` dict sorts the directory by path: the sort's scratch,
/// half the rows or more, was allocated without a charge (8 bytes a row).
#[test]
fn opening_a_bundle_holds_nothing_the_meter_has_not_charged() {
    for rows in [1_000, 100_000] {
        let peak = open_peak(rows);
        assert!(
            peak <= 0,
            "{rows} rows: the heap rose {peak} bytes above the meter's charge"
        );
    }
}

/// The `objects` dict sorts the object table by path id: the sort's
/// scratch was allocated without a charge (4 bytes an object).
#[test]
fn parsing_a_serialized_file_holds_nothing_the_meter_has_not_charged() {
    for objects in [1_000, 100_000] {
        let peak = parse_peak(objects);
        assert!(
            peak <= 0,
            "{objects} objects: the heap rose {peak} bytes above the meter's charge"
        );
    }
}

/// A run that covers cached runs collected them into a list, 48 bytes a
/// run, to drop them after the lock: that list was not charged.
#[test]
fn a_run_that_covers_cached_runs_holds_nothing_the_meter_has_not_charged() {
    for runs in [100, 10_000] {
        let peak = cover_peak(runs);
        assert!(
            peak <= 0,
            "{runs} runs: the heap rose {peak} bytes above the meter's charge"
        );
    }
}
