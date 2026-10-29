//! Package C's files under a counting allocator (plan sections 3.15 and
//! 7.4): after every step of opening, loading and reading the testkit
//! matrix's bundles and files, a bundle of many directory rows over one
//! minimal entry whose version string is the longest `read_string_to_null`
//! reads whole (and one whose own string is empty, taking the bundle's engine
//! string of that length), and a bundle of one large LZMA block, read eagerly
//! and lazily, the heap the core holds is at most what the input's meter
//! charged, plus the meter's own constant; everything returns when the
//! handles drop. Run under both budgets. One test, so nothing else allocates
//! while it runs.
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
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, matrix};
use unityrs_testkit::serialized::{SerializedSpec, build as file, one_object};

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

/// Opens `data` on a fresh meter of `limit` and reads it eagerly
/// (`load_all`, then every entry) or lazily (every entry's bytes and file in
/// directory order, each kept, as an Environment keeps what it parsed),
/// checking the heap after every step; then drops everything.
fn load(name: &str, data: &Bytes, ctx: &Arc<FileContext>, limit: u64, eager: bool) {
    // The caller's name and handle are the caller's heap, made first. A
    // `Bytes` made from a `Vec` allocates its shared header (three words) on
    // its first clone; cloning both here makes that the caller's too, as it
    // is for a caller that keeps its own handle. The list the reads keep
    // their values in is the caller's as well, and so is an error a read
    // returns (dropped at once).
    let path = RawStr::from(name);
    let data = data.clone();
    let caller = (path.clone(), data.clone());
    let mut kept: Vec<(Option<Bytes>, Option<File>)> = Vec::with_capacity(4096);
    let base = live();
    let m = Meter::new(limit, ctx.budget.clone(), Reservation::Input);
    let Ok(ty) = check_file_type(&data, &ctx.limits) else {
        return;
    };
    within(base, &m, name, "detect", 0);
    // A parse error is returned to the caller: the caller's heap, dropped.
    let parsed = parse_file(path, data, ty, None, &m, ctx).ok();
    within(base, &m, name, "parse", 0);
    if let Some(File::Bundle(b)) = &parsed {
        if eager {
            let got = b.load_all();
            within(base, &m, name, "load_all", 0);
            if let Err(Error::Limit(l)) = &got {
                assert_eq!(l.kind, LimitKind::InputMemory, "{name}");
            }
        }
        for i in 0..b.directory().len().min(kept.capacity()) {
            kept.push((b.entry_bytes(i).ok(), b.entry(i).ok()));
            within(
                base,
                &m,
                name,
                if eager { "eager entry" } else { "lazy entry" },
                i,
            );
        }
    }
    kept.clear();
    drop(parsed);
    assert_eq!(m.used(), 0, "{name}: every charge returned");
    drop(m);
    assert_eq!(live(), base, "{name}: every allocation returned");
    drop(caller);
}

/// The longest string `read_string_to_null` (window 32,767) reads whole with
/// its terminator: 32,766 bytes, a valid version with a long tail.
fn long_version() -> Vec<u8> {
    let mut v = b"2020.3.33f1".to_vec();
    v.resize(32_766, b'x');
    v
}

/// A bundle of `rows` directory rows over the same bytes: one minimal
/// `SerializedFile` whose own version is `own`, under the engine string
/// `engine`. Built with `rows` entries of which only the first has bytes,
/// then every row patched to the first's span (blocks-info stored, so the
/// rows are in the file as written).
fn rows_over_one_entry(rows: usize, own: Vec<u8>, engine: Vec<u8>) -> Vec<u8> {
    let entry = file(&SerializedSpec {
        unity_version: own,
        ..one_object(1, vec![NodeSpec::new(0, "GameObject", "Base")], vec![0; 4])
    });
    let size = i64::try_from(entry.len()).unwrap();
    let mut entries = vec![EntrySpec {
        name: b"CAB-00000".to_vec(),
        data: entry,
        flags: 4,
    }];
    entries.extend((1..rows).map(|i| EntrySpec {
        name: format!("CAB-{i:05}").into_bytes(),
        data: Vec::new(),
        flags: 4,
    }));
    let mut data = build(&BundleSpec {
        version_engine: engine,
        info_codec: TestCodec::None,
        block_size: 1 << 17,
        entries,
        ..BundleSpec::default()
    });
    for i in 1..rows {
        let key = format!("CAB-{i:05}\0").into_bytes();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        data[at - 20..at - 12].copy_from_slice(&0i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    data
}

/// Every row's version is a slice of the one entry's bytes (or of the
/// bundle's engine string), never a copy, and the eager load holds far less
/// than a copy per row would.
fn versions_are_shared(name: &str, data: &[u8], rows: usize) {
    let ctx = Arc::new(FileContext::unitypy());
    let m = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let data = Bytes::copy_from_slice(data);
    let ty = check_file_type(&data, &ctx.limits).unwrap();
    let Ok(File::Bundle(b)) = parse_file(RawStr::from(name), data, ty, None, &m, &ctx) else {
        panic!("{name}: not a bundle");
    };
    b.load_all().unwrap();
    let mut at = None;
    for i in 0..rows {
        let Ok(File::Serialized(s)) = b.entry(i) else {
            panic!("{name}: row {i}");
        };
        let raw = s.version().unwrap().version.raw();
        assert_eq!(raw.as_bytes().len(), 32_766, "{name}: row {i}");
        let p = raw.as_bytes().as_ptr();
        assert_eq!(
            *at.get_or_insert(p),
            p,
            "{name}: row {i} shares row 0's string"
        );
    }
    assert!(
        m.used() < u64::try_from(rows * 32_766 / 4).unwrap(),
        "{name}: {} held for {rows} rows",
        m.used()
    );
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
    let own = rows_over_one_entry(1000, long_version(), b"2020.3.33f1".to_vec());
    let engine = rows_over_one_entry(1000, Vec::new(), long_version());
    versions_are_shared("own-version.bundle", &own, 1000);
    versions_are_shared("engine-version.bundle", &engine, 1000);
    inputs.push(("own-version.bundle".into(), Bytes::from(own)));
    inputs.push(("engine-version.bundle".into(), Bytes::from(engine)));
    // One LZMA block larger than the small limit below: refused before its
    // output outgrows the meter.
    let noise: Vec<u8> = (0..4u32 << 20)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24).to_le_bytes()[0])
        .collect();
    inputs.push((
        "large-lzma.bundle".into(),
        Bytes::from(build(&BundleSpec {
            data_codec: TestCodec::Lzma,
            block_size: 8 << 20,
            entries: vec![EntrySpec {
                name: b"big".to_vec(),
                data: noise,
                flags: 0,
            }],
            ..BundleSpec::default()
        })),
    ));
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
            for eager in [true, false] {
                load(name, data, &ctx, u64::MAX, eager);
                // And at a small limit, where the hostile ones stop.
                load(name, data, &ctx, 1 << 20, eager);
            }
        }
    }
}
