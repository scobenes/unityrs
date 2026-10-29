//! The memory model of plan section 3.15, measured: everything the core
//! allocates in proportion to what an input chooses is charged to the input's
//! meter first, so one input holds at most the meter's limit plus a constant.
//!
//!     scripts/dev cargo test -p unityrs-core --test memory_model -- --nocapture
//!
//! A counting global allocator watches the heap while each path runs, and
//! checks two things: the heap never holds more than the meter has charged,
//! with no slack, and the two inputs round 2 of the review built,
//! overlapping directory entries and lazily re-decoded block spans, now stop
//! with `LimitKind::InputMemory` at the meter's limit. They are run here at
//! a 64 MiB limit, scaled down from the reviewers' 13 GiB and 748 GiB, through
//! the same scaffold types the parsers must use (`MeteredVec` for every
//! table, `DecodeBuf` for every decode, `ValueBuilder` for every value).
//!
//! Every path runs twice: with the meter booking an `Unbounded` budget
//! (UnityPy mode) and a `Counting` one (a batch's pools), whose permits have
//! a home, so the owners' bookkeeping is measured too (round 3: under
//! `Counting` each charge left an uncharged permit record, and a value's heap
//! reached twice its charge). This file holds one test so nothing else
//! allocates while it runs.
//!
//! **The heap is counted in glibc chunks**, [`heap_cost`] of each request
//! (checked against `malloc_usable_size` below), the same unit the meter
//! charges. Counting requested sizes instead would leave every charged
//! allocation's rounding (8 to 23 bytes, a page for an mmapped chunk) as
//! room in which an uncharged allocation could hide: an 8-byte box per list
//! costs a 32-byte chunk and would pass (round 4).

#![allow(unsafe_code, clippy::cast_precision_loss)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::budget::{
    Counting as CountingBudget, DecodeBuf, MemoryBudget, Reservation, Unbounded,
};
use unityrs_core::limits::{ChargeError, Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::typetree::{NodeId, NodeList, NodeRow};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr, Value, ValueBuilder};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The glibc chunk a request of `n` bytes takes (module docs).
fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

// SAFETY: every method forwards to `System` and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let c = chunk(layout.size());
        let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Count the worst case of a moving realloc: both blocks live at once.
        let c = chunk(new_size);
        let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        LIVE.fetch_sub(chunk(layout.size()), Ordering::Relaxed);
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const MIB: u64 = 1 << 20;
/// The scaled-down meter limit of the two hostile inputs.
const LIMIT: u64 = 64 * MIB;

fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

fn live() -> u64 {
    bytes(LIVE.load(Ordering::Relaxed))
}

fn reset_peak() -> u64 {
    let base = live();
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    base
}

fn peak_above(base: u64) -> u64 {
    bytes(PEAK.load(Ordering::Relaxed)).saturating_sub(base)
}

/// A meter of `limit` that books with an `Unbounded` budget, or with a
/// `Counting` one whose permits have a home.
fn meter(limit: u64, counting: bool) -> Arc<Meter> {
    let budget: Arc<dyn MemoryBudget> = if counting {
        Arc::new(CountingBudget::new(u64::MAX))
    } else {
        Arc::new(Unbounded)
    };
    Meter::new(limit, budget, Reservation::Input)
}

/// The heap above `base`, in glibc chunks, is at most what `m` charged.
fn held_within_charge(base: u64, m: &Meter, what: &str) {
    let live = live().saturating_sub(base);
    assert!(
        live <= m.used(),
        "{what}: heap {live} above the charge {}",
        m.used()
    );
}

fn assert_limit(e: &ChargeError) {
    assert!(
        matches!(e, ChargeError::Limit(l) if l.kind == LimitKind::InputMemory && l.max == LIMIT),
        "{e:?}"
    );
}

/// Round 2's overlapping directory entries: K rows over the same bytes,
/// each parse keeping its own copy of a type's dependency list (a
/// `read_i32_array` that passed its `4 * count <= remaining` check). Every
/// parse grows the table on the input's meter, so the copies stop at the
/// limit, however many rows point at the bytes.
fn overlapping_entries(counting: bool) -> usize {
    const K: usize = 12;
    const COUNT: usize = 2 << 20; // 8 MiB of i32 per parse
    let m = meter(LIMIT, counting);
    let mut parsed: Vec<MeteredVec<i32>> = Vec::with_capacity(K);
    let base = reset_peak();
    let mut refused = None;
    for _ in 0..K {
        let mut deps = match MeteredVec::with_capacity(&m, COUNT) {
            Ok(v) => v,
            Err(e) => {
                refused = Some(e);
                break;
            }
        };
        for i in 0..COUNT {
            deps.push(i32::try_from(i & 0xff).unwrap()).unwrap();
        }
        parsed.push(deps);
        held_within_charge(base, &m, "overlapping entries");
    }
    assert_limit(&refused.expect("the meter refused a copy"));
    assert!(
        peak_above(base) <= LIMIT,
        "peak {} above the limit",
        peak_above(base)
    );
    let kept = parsed.len();
    drop(parsed);
    assert_eq!(m.used(), 0, "the parses return their charges");
    kept
}

/// Round 2's lazy runs: 32 blocks, a directory of every contiguous span
/// sorted shortest first, read in directory order, each read decoding its
/// span into a new cached run (no shorter run covers a longer span). The
/// spans need 5,984 block decodes; every decode is charged, so the cache
/// stops at the limit. One eager run of all 32 blocks fits the same meter.
fn lazy_runs(counting: bool) -> (usize, u64) {
    const BLOCKS: usize = 32;
    const BLOCK: usize = 256 << 10;
    let m = meter(LIMIT, counting);
    let mut spans: Vec<(usize, usize)> = (0..BLOCKS)
        .flat_map(|i| ((i + 1)..=BLOCKS).map(move |j| (i, j)))
        .collect();
    spans.sort_by_key(|(i, j)| (j - i, *i));
    let needed: usize = spans.iter().map(|(i, j)| j - i).sum();
    assert_eq!((spans.len(), needed), (528, 5984));
    let mut runs: Vec<Bytes> = Vec::with_capacity(spans.len());
    let base = reset_peak();
    let mut refused = None;
    'reads: for (i, j) in &spans {
        // A run: one segment per block, each capped at max_block_output.
        let mut out = match DecodeBuf::new(&m, 0, bytes(BLOCK), LimitKind::BlockOutput) {
            Ok(b) => b,
            Err(e) => {
                refused = Some(e);
                break;
            }
        };
        for _ in *i..*j {
            out.begin_block(bytes(BLOCK), LimitKind::BlockOutput);
            // The codec writes the block's output (an LZMA block of zeros).
            if let Err(e) = out.grow_zeroed(BLOCK) {
                refused = Some(e);
                break 'reads;
            }
        }
        match out.into_bytes() {
            Ok(run) => runs.push(run),
            Err(e) => {
                refused = Some(e);
                break;
            }
        }
        held_within_charge(base, &m, "lazy runs");
    }
    assert_limit(&refused.expect("the meter refused a run"));
    assert!(
        peak_above(base) <= LIMIT,
        "peak {} above the limit",
        peak_above(base)
    );
    let cached = runs.len();
    drop(runs);
    assert_eq!(m.used(), 0, "the runs return their charges");

    // Eager: every block once, into one run, under the same meter.
    let mut all = DecodeBuf::new(&m, 0, bytes(BLOCK), LimitKind::BlockOutput).unwrap();
    for _ in 0..BLOCKS {
        all.begin_block(bytes(BLOCK), LimitKind::BlockOutput);
        all.grow_zeroed(BLOCK).unwrap();
    }
    let joined = all.into_bytes().unwrap();
    let held = m.used();
    assert_eq!(joined.len(), BLOCKS * BLOCK);
    drop(joined);
    (cached, held)
}

/// Typetree rows: the parser's row table and every table `from_rows` builds
/// are charged, in both shapes (a flat root, and a chain, the deepest).
fn typetree_rows(counting: bool) -> f64 {
    let n: u32 = 1 << 18;
    let strings = Bytes::from(b"Base\0int\0m_Name\0".to_vec());
    let (ty, name) = (strings.slice(5..8), strings.slice(9..15));
    let mut per_row: f64 = 0.0;
    for chain in [false, true] {
        let m = meter(u64::MAX, counting);
        let base = reset_peak();
        let mut rows = MeteredVec::new(&m);
        for i in 0..n {
            rows.push(NodeRow {
                level: if chain {
                    i32::try_from(i).unwrap()
                } else {
                    i32::from(i != 0)
                },
                ty: RawStr::from_shared(ty.clone()),
                name: RawStr::from_shared(name.clone()),
                index: Some(0),
                meta_flag: Some(0),
                type_flags: Some(0),
                ..NodeRow::default()
            })
            .unwrap();
        }
        let list = NodeList::from_rows(rows).unwrap();
        held_within_charge(base, &m, "typetree rows");
        assert!(
            peak_above(base) <= m.used() * 2,
            "from_rows's peak above twice what it keeps"
        );
        assert_eq!(list.charge().bytes(), m.used());
        per_row = per_row.max(m.used() as f64 / f64::from(n));
        drop(list);
        assert_eq!(m.used(), 0);
    }
    per_row
}

/// Values: a `Parsed` carries the charge for every allocation of its value
/// and for its table of node lists, so a value a caller keeps is still on
/// the meter, and dropping it returns it. Three shapes, every value kept:
/// 16,384 maps of two entries in one list (plan 3.10's shape); 262,144
/// one-element lists (round 3's `vector<vector<int>>`, the most charges per
/// input byte); and 4,096 objects each reading through 64 distinct ref-type
/// trees (round 3: `Parsed::lists` was allocated by its caller and charged
/// by nobody).
fn parsed_values(counting: bool) -> u64 {
    let m = meter(u64::MAX, counting);
    let list = Arc::new(
        NodeList::trusted(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "a"),
            NodeRow::new(1, "int", "b"),
        ])
        .unwrap(),
    );
    let refs: Vec<Arc<NodeList>> = (0..64)
        .map(|_| Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Ref", "Base")]).unwrap()))
        .collect();
    let mut kept: Vec<Parsed> = Vec::with_capacity(4096 + 2);
    let base = reset_peak();

    let mut b = ValueBuilder::new(&m, list.clone()).unwrap();
    let mut items = b.vec();
    for i in 0..(1 << 14) {
        let mut entries = b.vec();
        entries.reserve_exact(2).unwrap();
        entries.push((NodeId(1), Value::Int(i))).unwrap();
        entries.push((NodeId(2), Value::Int(-i))).unwrap();
        let map = b.map(0, entries);
        items.push(map).unwrap();
    }
    let value = b.list(items);
    kept.push(b.finish(value, 0));
    held_within_charge(base, &m, "maps");

    let mut b = ValueBuilder::new(&m, list.clone()).unwrap();
    let mut outer = b.vec();
    for i in 0..(1 << 18) {
        let mut inner = b.vec();
        inner.push(Value::Int(i)).unwrap();
        let one = b.list(inner);
        outer.push(one).unwrap();
    }
    let value = b.list(outer);
    kept.push(b.finish(value, 0));
    held_within_charge(base, &m, "one-element lists");

    for _ in 0..4096 {
        let mut b = ValueBuilder::new(&m, list.clone()).unwrap();
        let mut entries = b.vec();
        for (k, r) in refs.iter().enumerate() {
            let ix = b.list_index(r).unwrap();
            let data = b.map(ix, b.vec());
            entries
                .push((NodeId(u32::try_from(k).unwrap()), data))
                .unwrap();
        }
        let value = b.map(0, entries);
        let p = b.finish(value, 0);
        assert_eq!(p.lists().len(), 65);
        kept.push(p);
    }
    held_within_charge(base, &m, "ref-type lists");
    for p in &kept {
        assert_eq!(p.charge().records(), 0, "one permit per value");
    }

    let held = m.used();
    drop(kept);
    assert_eq!(m.used(), 0, "dropping the values returns their charge");
    held
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap_cost_covers_glibc() {
    unsafe extern "C" {
        fn malloc_usable_size(ptr: *mut std::ffi::c_void) -> usize;
    }
    // Round 3: requests of 4096k - 23 ..= 4096k - 16 bytes, and those whose
    // padded chunk first reaches the mmap threshold, take a page more than
    // the old formula charged.
    let mut sizes = vec![
        1_usize,
        8,
        24,
        25,
        100,
        1000,
        4096,
        100_000,
        131_072,
        1 << 20,
        5 << 20,
    ];
    for k in [32_usize, 33, 49, 1024, 1025] {
        sizes.extend((0..=40).map(|r| 4096 * k - r));
    }
    sizes.extend(131_032..=131_080);
    for n in sizes {
        let layout = Layout::from_size_align(n, 8).unwrap();
        // SAFETY: a non-zero layout; the block is freed below.
        let p = unsafe { System.alloc(layout) };
        assert!(!p.is_null());
        // SAFETY: `p` came from glibc's malloc (the system allocator).
        let usable = unsafe { malloc_usable_size(p.cast()) };
        // SAFETY: allocated above with this layout.
        unsafe { System.dealloc(p, layout) };
        // A chunk is its usable size plus its header: 8 bytes, or 16 for an
        // mmapped chunk, which only a padded request of 128 KiB or more can
        // be (counted as 16 whether or not it was).
        let padded = (n + 23) & !15;
        let header = if padded < 128 << 10 { 8 } else { 16 };
        assert!(
            bytes(usable + header) <= heap_cost(bytes(n)),
            "{n}: glibc's chunk {} above heap_cost {}",
            usable + header,
            heap_cost(bytes(n))
        );
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn heap_cost_covers_glibc() {}

#[test]
fn the_meter_bounds_every_path() {
    heap_cost_covers_glibc();

    for counting in [false, true] {
        let budget = if counting { "Counting" } else { "Unbounded" };
        let kept = overlapping_entries(counting);
        println!(
            "[{budget}] overlapping entries: {kept} of 12 parses kept at a {} MiB meter, then InputMemory",
            LIMIT / MIB
        );
        assert_eq!(kept, 7);

        let (cached, eager) = lazy_runs(counting);
        println!(
            "[{budget}] lazy runs: {cached} of 528 spans cached at a {} MiB meter, then InputMemory; one eager run holds {} MiB",
            LIMIT / MIB,
            eager / MIB
        );
        assert!(cached < 528);

        let per_row = typetree_rows(counting);
        println!(
            "[{budget}] typetree rows: at most {per_row:.1} bytes charged per row (row table and child tables)"
        );

        let held = parsed_values(counting);
        println!(
            "[{budget}] parsed values: maps, one-element lists and ref-type lists hold {held} bytes, all charged, all returned"
        );
    }

    // The meter's own heap, as the constant says: its account and the meter,
    // then one more meter per reopen.
    let budget: Arc<dyn MemoryBudget> = Arc::new(Unbounded);
    let base = live();
    let first = Meter::new(u64::MAX, budget, Reservation::Dependency);
    assert_eq!(live() - base, Meter::OWN_HEAP);
    let again = first.reopen();
    assert_eq!(live() - base, Meter::OWN_HEAP + Meter::REOPEN_HEAP);
    drop((first, again));

    let l = Limits::default();
    // Not charged, and not in proportion to anything the input chooses: the
    // meter itself and its memory account (one per input, and one more meter
    // per reopen of an evicted dependency), and the stack of a call bounded
    // by the depth limits. The provider cache is per context, bounded on its
    // own.
    println!(
        "bound per input: max_input_memory {} GiB + {} bytes (the meter and its account; {} more per reopen) + per reading thread its stack and detection's fixed scratch; per context the provider cache, at most {} MiB",
        l.max_input_memory >> 30,
        Meter::OWN_HEAP,
        Meter::REOPEN_HEAP,
        unityrs_core::provider::ProviderChain::DEFAULT_CACHE_BYTES >> 20
    );
    assert_eq!(l.max_input_memory, 8 << 30);
}
