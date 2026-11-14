//! The provider's tables under a counting allocator. `docs/plans/core.md`
//! section 3.15: what is allocated in proportion to an input-chosen size is
//! charged first, a charge being `heap_cost` of the request (a `MeteredVec`
//! at `heap_cost` of its capacity), and a TPK's tables are "charged to its
//! own meter as they grow" (the bound's paragraph); crate docs, "Memory".
//! So at every allocation a stored TPK's load, a walk and a compile make,
//! the heap above the run's start is at most what the provider's meter has
//! charged since, in glibc's chunks and with no slack, as `unityrs-core`'s
//! `tests/walk_memory.rs` holds core's reads (review round 16: moving any of
//! fourteen of the provider's tables off its meter passed every test). A
//! compressed TPK's load is not watched: its decoded payload is charged to
//! its decoder's own meter, not the provider's (`file::decode`).
//!
//! A string the blob reads is charged its owner record at `OWNER_COST`
//! (`unityrs_core::budget`), a ceiling over the record `Bytes::from_owner`
//! boxes, so once a TPK's strings are read the meter is ahead of the heap by
//! the difference for each, and a table built after them could pass
//! uncharged within it. So the load is watched twice: of a TPK of named
//! strings, and of the same TPK with every string empty, which charges and
//! allocates nothing (`RawStr::charged`): there the meter is never ahead of
//! the heap at an allocation (asserted), and every table is held to its
//! own charge.
//!
//! This module installs the counting allocator for the crate's unit tests;
//! it counts only the thread under test (`unityrs_testkit::counted`).

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::UnityVersion;
use unityrs_core::limits::{Limits, Meter, heap_cost};
use unityrs_core::provider::{TreeRequest, TypeTreeProvider};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

use super::{TpkProvider, provider_meter};

/// The counted thread's heap, in glibc chunks.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The heap when the watched run started.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The watched meter's charge when the run started.
static START: AtomicUsize = AtomicUsize::new(0);
/// The most the heap above `BASE` exceeded what the meter charged since.
static EXCESS: AtomicUsize = AtomicUsize::new(0);
/// The request at which the heap first exceeded the charge, or 0.
static FIRST: AtomicUsize = AtomicUsize::new(0);
/// The most the meter's charge since the start was ahead of the heap
/// above `BASE`, at an allocation.
static AHEAD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The meter being watched: the test thread's, the only one counted.
    static WATCHED: Cell<Option<&'static Meter>> = const { Cell::new(None) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    // A second counted thread must not move the table watch's global heap.
    static COUNT_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// The glibc chunk a request of `n` bytes takes (`heap_cost`).
fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(u64::try_from(n).unwrap_or(u64::MAX))).unwrap_or(usize::MAX)
}

struct Counting;

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get().saturating_add(1)));
        if COUNT_ONLY.try_with(Cell::get).unwrap_or(false) {
            return;
        }
        let c = chunk(size);
        let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
        let Some(m) = WATCHED.try_with(Cell::get).ok().flatten() else {
            return;
        };
        let used = usize::try_from(m.used()).unwrap_or(usize::MAX);
        let heap = now.saturating_sub(BASE.load(Ordering::Relaxed));
        let charged = used.saturating_sub(START.load(Ordering::Relaxed));
        let over = heap.saturating_sub(charged);
        if over > 0 {
            let _ = FIRST.compare_exchange(0, size, Ordering::Relaxed, Ordering::Relaxed);
        }
        EXCESS.fetch_max(over, Ordering::Relaxed);
        AHEAD.fetch_max(charged.saturating_sub(heap), Ordering::Relaxed);
    }

    fn shrank(&self, size: usize) {
        if COUNT_ONLY.try_with(Cell::get).unwrap_or(false) {
            return;
        }
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// Ends a watch when it drops, a failed assertion's unwind included.
struct Watch;

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = WATCHED.try_with(|w| w.set(None));
    }
}

/// `f`, run with every allocation it makes checked against what `meter`
/// charged since it started; panics naming `what` at the first excess.
/// Afterwards `AHEAD` holds the most the charge was ahead of the heap.
fn watching<T>(meter: &'static Meter, what: &str, f: impl FnOnce() -> T) -> T {
    START.store(usize::try_from(meter.used()).unwrap(), Ordering::Relaxed);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    EXCESS.store(0, Ordering::Relaxed);
    FIRST.store(0, Ordering::Relaxed);
    AHEAD.store(0, Ordering::Relaxed);
    let watch = Watch;
    WATCHED.with(|w| w.set(Some(meter)));
    let out = f();
    drop(watch);
    let (excess, first) = (
        EXCESS.load(Ordering::Relaxed),
        FIRST.load(Ordering::Relaxed),
    );
    assert_eq!(
        excess, 0,
        "{what}: the heap passed the provider's charge by {excess} bytes, first at a request of {first} bytes"
    );
    out
}

/// `major.0.0f1`, packed as a TPK stores it.
fn version(major: u64) -> u64 {
    (major << 48) | (3 << 8) | 1
}

/// A stored TPK whose every table holds many records: 64 versions; 48
/// classes, each with three entries (removed at 2000, both roots at 2010,
/// a release root alone at 2020: nodes `c` and `48 + c`); 8 common-string
/// versions and 64 indices; 300 nodes, 0-99 each listing eight of the
/// leaves 100-299; 120 strings, of 1 to 30 bytes when `named`, else empty.
fn tpk(named: bool) -> Vec<u8> {
    let n = |n: usize| i32::try_from(n).unwrap().to_le_bytes();
    let id = |n: usize| u16::try_from(n).unwrap().to_le_bytes();
    let mut blob = 1_i64.to_le_bytes().to_vec();
    blob.extend_from_slice(&n(64));
    for m in 0..64 {
        blob.extend_from_slice(&version(1990 + m).to_le_bytes());
    }
    blob.extend_from_slice(&n(48));
    for c in 0..48 {
        blob.extend_from_slice(&n(c));
        blob.extend_from_slice(&n(3));
        blob.extend_from_slice(&version(2000).to_le_bytes());
        blob.push(0);
        // Present, its name and base, then flags 192 (both roots) or 128.
        blob.extend_from_slice(&version(2010).to_le_bytes());
        blob.extend_from_slice(&[1, 7, 0, 0, 0, 192]);
        blob.extend_from_slice(&id(c));
        blob.extend_from_slice(&id(c));
        blob.extend_from_slice(&version(2020).to_le_bytes());
        blob.extend_from_slice(&[1, 7, 0, 0, 0, 128]);
        blob.extend_from_slice(&id(48 + c));
    }
    blob.extend_from_slice(&n(8));
    for m in 0..8 {
        blob.extend_from_slice(&version(2000 + m).to_le_bytes());
        blob.push(u8::try_from(m).unwrap());
    }
    blob.extend_from_slice(&n(64));
    for i in 0..64 {
        blob.extend_from_slice(&id(i));
    }
    blob.extend_from_slice(&n(300));
    for i in 0..300 {
        // `<HHihbIH`: type and name strings, size -1, version 1, flags 0.
        blob.extend_from_slice(&id(i % 120));
        blob.extend_from_slice(&id((i + 1) % 120));
        blob.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0]);
        let subs: Vec<usize> = if i < 100 {
            (0..8).map(|k| 100 + (i * 8 + k) % 200).collect()
        } else {
            Vec::new()
        };
        blob.extend_from_slice(&id(subs.len()));
        for s in subs {
            blob.extend_from_slice(&id(s));
        }
    }
    blob.extend_from_slice(&n(120));
    for i in 0..120 {
        let len = if named { 1 + i % 30 } else { 0 };
        blob.push(u8::try_from(len).unwrap());
        blob.extend((0..len).map(|k| b'a' + u8::try_from((i + k) % 26).unwrap()));
    }
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

/// A load (every table of the blob, the common strings, the release roots
/// and what `builds` finds of each, the name table), then for four release
/// roots, of both kinds of entry, a walk (its rows, its stack and their
/// list) and a compile, each under the watch: no allocation of any is above
/// the provider's charge. Then the load of the TPK with empty strings, where
/// the meter is never ahead of the heap (module docs).
#[test]
fn every_table_the_provider_builds_is_charged_before_it_is_allocated() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let bytes = tpk(true);
    let limits = Limits::default();
    let requests = [(0, 2010), (7, 2010), (23, 2020), (47, 2020)];
    // A first run, unwatched, makes what a process makes once (a static
    // built lazily), so the watched run sees only what it makes for the TPK.
    let first = TpkProvider::load(&bytes, &limits, provider_meter()).unwrap();
    for (class, at) in requests {
        let v = UnityVersion::from_packed(version(at));
        let req = TreeRequest {
            class_id: class,
            version: &v,
            script: None,
        };
        first.tree(&req).unwrap().unwrap();
    }
    drop(first);
    let meter = provider_meter();
    let watched: &'static Meter = Box::leak(Box::new(meter.clone()));
    let provider = watching(watched, "the load", || {
        TpkProvider::load(&bytes, &limits, meter.clone()).unwrap()
    });
    assert_eq!(provider.class_count(), 48);
    assert_eq!(provider.versions().len(), 64);
    assert_eq!(provider.common.len(), 64);
    assert_eq!(provider.blob.strings.len(), 120);
    assert_eq!(provider.blob.nodes.len(), 300);
    assert_eq!(provider.roots.len(), 96);
    for (class, at) in requests {
        let v = UnityVersion::from_packed(version(at));
        let what = format!("class {class} at {at}");
        let list = watching(watched, &format!("{what}: the walk"), || {
            provider.node_list(class, &v).unwrap().unwrap()
        });
        assert_eq!(list.rows().len(), 9, "{what}");
        let req = TreeRequest {
            class_id: class,
            version: &v,
            script: None,
        };
        let tree = watching(watched, &format!("{what}: the compile"), || {
            provider.tree(&req).unwrap().unwrap()
        });
        assert!(Arc::ptr_eq(tree.list(), &list), "{what}");
    }
    let bytes = tpk(false);
    let meter = provider_meter();
    let watched: &'static Meter = Box::leak(Box::new(meter.clone()));
    let provider = watching(watched, "the load of empty strings", || {
        TpkProvider::load(&bytes, &limits, meter.clone()).unwrap()
    });
    assert_eq!(
        AHEAD.load(Ordering::Relaxed),
        0,
        "the load of empty strings: the meter ran ahead of the heap, so a table could hide in it"
    );
    assert_eq!(provider.blob.strings.len(), 120);
    assert_eq!(provider.common.len(), 64);
    assert_eq!(provider.roots.len(), 96);
}

#[test]
fn allocator_load_errors_keep_provenance_without_allocating() {
    use unityrs_core::{
        budget::{AllocError, Reservation},
        limits::ChargeError,
    };
    COUNT_ONLY.with(|c| c.set(true));
    count_this_thread();
    let a = AllocError {
        requested: 4096,
        what: Reservation::Provider,
    };
    let before = ALLOCATIONS.with(Cell::get);
    let error = crate::blob::memory(ChargeError::Allocator(a));
    assert_eq!(
        ALLOCATIONS.with(Cell::get),
        before,
        "refusal conversion allocates nothing"
    );
    assert!(matches!(error, crate::TpkError::Allocator(got) if got == a));
}
