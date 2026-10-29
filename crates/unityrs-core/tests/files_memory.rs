//! Package C's files under a counting allocator (plan sections 3.15 and
//! 7.4): after every step of opening, loading and reading the testkit
//! matrix's bundles and files, a bundle of many directory rows over one
//! minimal entry whose version string is the longest `read_string_to_null`
//! reads whole (and one whose own string is empty, taking the bundle's engine
//! string of that length), and a bundle of one large LZMA block, read eagerly
//! and lazily, the heap the core holds is at most what the input's meter
//! charged, plus the meter's own constant; everything returns when the
//! handles drop; and a failure a bundle keeps for an entry hands out clones
//! that share its heap. Run under both budgets. The allocator counts only the test's
//! own thread (`unityrs_testkit::counted`): libtest's main thread allocates
//! beside it.
//!
//!     scripts/dev cargo test -p unityrs-core --test files_memory

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::budget::{Counting as CountingBudget, MemoryBudget, Reservation, Unbounded};
use unityrs_core::codec::BlockDecryptor;
use unityrs_core::detect::check_file_type;
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::Rng;
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, matrix};
use unityrs_testkit::counted::{count_this_thread, counted, others_are_not_counted};
use unityrs_testkit::serialized::{SerializedSpec, build as file, one_object};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

// SAFETY: every method forwards to `System` and only counts sizes, on the
// thread under test.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counted() {
            LIVE.fetch_add(chunk(layout.size()), Ordering::Relaxed);
        }
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
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
        if on {
            LIVE.fetch_add(chunk(new_size), Ordering::Relaxed);
        }
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if on {
            LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

thread_local! {
    /// The open being checked: the heap baseline and its meter.
    static OPEN: RefCell<Option<(u64, Arc<Meter>)>> = const { RefCell::new(None) };
}

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
    OPEN.with(|o| *o.borrow_mut() = Some((base, m.clone())));
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
    OPEN.with(|o| *o.borrow_mut() = None);
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

/// A decryptor that XORs every byte with its key.
/// While it runs it checks the heap against the open's meter, so a copy of
/// the block the core did not charge fails here, not only at a checkpoint.
struct Xor(u8);

impl BlockDecryptor for Xor {
    fn decrypt(&self, block: &mut [u8], index: usize) -> Result<(), Error> {
        OPEN.with(|o| {
            if let Some((base, m)) = &*o.borrow() {
                within(*base, m, "decrypt", "block", index);
            }
        });
        for b in block {
            *b ^= self.0;
        }
        Ok(())
    }
}

/// A bundle of one LZ4 block of noise flagged `0x100` (encrypted), each
/// stored byte exclusive-or `0x5A`, for a context with [`Xor`].
fn flagged_bundle() -> Vec<u8> {
    let mut rng = Rng::new(11);
    let entry: Vec<u8> = (0..200_000)
        .map(|_| rng.next_u64().to_le_bytes()[0])
        .collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"secret".to_vec(),
            data: entry,
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + data[from..].iter().position(|c| *c == 0).unwrap() + 1;
    let row = (nul(nul(12)) + 20).next_multiple_of(16) + 20;
    let stored = usize::try_from(u32::from_be_bytes(
        data[row + 4..row + 8].try_into().unwrap(),
    ))
    .unwrap();
    let flags = u16::from_be_bytes([data[row + 8], data[row + 9]]) | 0x100;
    data[row + 8..row + 10].copy_from_slice(&flags.to_be_bytes());
    let len = data.len();
    for b in &mut data[len - stored..] {
        *b ^= 0x5A;
    }
    data
}

/// Codex's review, round 1: a bundle keeps an entry's failure in a cell,
/// charged once, and hands every later ask a clone of it. Each clone shares
/// the kept error's heap, so a thousand asks, every answer kept, hold no
/// more heap than before them: a block that fails to decode
/// (`Error::Decompress`, its message) and an entry whose AssetBundle object
/// fails to read (`Error::Entry` around `Error::AssetBundle`, the read's
/// error).
fn kept_errors_are_shared(ctx: &Arc<FileContext>) {
    type Kind = fn(&Error) -> bool;
    let truncated = file(&one_object(
        142,
        vec![
            NodeSpec::new(0, "AssetBundle", "Base"),
            NodeSpec::new(1, "int", "m_x"),
        ],
        Vec::new(),
    ));
    let lz4 = matrix()
        .into_iter()
        .find(|(n, _)| n == "lz4-declared-smaller.bundle")
        .map(|(_, b)| b)
        .unwrap();
    let asset_bundle = build(&BundleSpec {
        entries: vec![EntrySpec {
            name: b"CAB-ab".to_vec(),
            data: truncated,
            flags: 4,
        }],
        ..BundleSpec::default()
    });
    let is_decompress: Kind = |e| matches!(e, Error::Decompress { .. });
    let is_asset_bundle: Kind =
        |e| matches!(e, Error::Entry { source, .. } if matches!(**source, Error::AssetBundle(_)));
    let cases = [
        ("lz4-declared-smaller.bundle", lz4, is_decompress),
        ("asset-bundle.bundle", asset_bundle, is_asset_bundle),
    ];
    for (name, data, kind) in cases {
        let data = Bytes::from(data);
        let _shared = data.clone();
        let m = Meter::new(u64::MAX, ctx.budget.clone(), Reservation::Input);
        let ty = check_file_type(&data, &ctx.limits).unwrap();
        let Ok(File::Bundle(b)) = parse_file(RawStr::from(name), data, ty, None, &m, ctx) else {
            panic!("{name}: not a bundle");
        };
        let first = b.entry(0).unwrap_err();
        assert!(kind(&first), "{name}: {first:?}");
        let mut answers = Vec::with_capacity(1000);
        let base = live();
        for _ in 0..1000 {
            answers.push(b.entry(0).unwrap_err());
        }
        let held = live() - base;
        assert_eq!(
            held, 0,
            "{name}: 1000 kept clones of the error hold {held} bytes"
        );
    }
}

#[test]
fn files_hold_no_more_than_their_meter() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
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
    let flagged = Bytes::from(flagged_bundle());
    for counting in [false, true] {
        let budget: Arc<dyn MemoryBudget> = if counting {
            Arc::new(CountingBudget::new(u64::MAX))
        } else {
            Arc::new(Unbounded)
        };
        let mut ctx = FileContext::unitypy();
        ctx.budget = budget;
        let mut decrypting = ctx.clone();
        decrypting.decryptor = Some(Arc::new(Xor(0x5A)));
        let ctx = Arc::new(ctx);
        let decrypting = Arc::new(decrypting);
        kept_errors_are_shared(&ctx);
        for eager in [true, false] {
            for (name, data) in &inputs {
                load(name, data, &ctx, u64::MAX, eager);
                // And at a small limit, where the hostile ones stop.
                load(name, data, &ctx, 1 << 20, eager);
            }
            // The decrypted copy of a block is charged too.
            load("flagged.bundle", &flagged, &decrypting, u64::MAX, eager);
            load("flagged.bundle", &flagged, &decrypting, 300_000, eager);
        }
    }
}
