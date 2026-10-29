//! The memory model of plan section 3.15, measured: the sizes of the parsed
//! records, the bytes a typetree row costs while `NodeList::from_rows` builds
//! a tree and after it, and the per-input bound the default `Limits` give.
//!
//!     scripts/dev cargo test -p unityrs-core --test memory_model -- --nocapture
//!
//! A counting global allocator measures the heap; this file holds one test so
//! nothing else allocates while it runs.

#![allow(unsafe_code, clippy::cast_precision_loss)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::files::{
    BlockInfo, DirEntry, External, FileSlot, ObjectInfo, ScriptRef, SerializedType,
};
use unityrs_core::limits::Limits;
use unityrs_core::typetree::{NodeId, NodeInfo, NodeList, NodeRow};
use unityrs_core::{Bytes, RawStr, Value};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` and only counts sizes.
unsafe impl GlobalAlloc for Counting {
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
        // Count the worst case of a moving realloc: both blocks live at once.
        let now = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const GIB: f64 = (1u64 << 30) as f64;

fn per(bytes: usize, n: usize) -> f64 {
    bytes as f64 / n as f64
}

#[test]
fn the_default_limits_hold_one_input_inside_12_gib() {
    // Rows as parse_blob makes them (strings are slices of one string buffer),
    // in two shapes: a flat root (one long child list) and a chain (one
    // small child list per row), the costliest for `from_rows`.
    let n = 1 << 20;
    let strings = Bytes::from(b"Base\0int\0m_Name\0".to_vec());
    let (ty, name) = (strings.slice(5..8), strings.slice(9..15));
    let mut peak_row: f64 = 0.0;
    let mut kept_row: f64 = 0.0;
    for chain in [false, true] {
        let rows: Vec<NodeRow> = (0..n)
            .map(|i| NodeRow {
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
            .collect();
        let base = LIVE.load(Ordering::Relaxed);
        PEAK.store(base, Ordering::Relaxed);
        let list = NodeList::from_rows(rows).unwrap();
        let held = n * size_of::<NodeRow>();
        peak_row = peak_row.max(per(PEAK.load(Ordering::Relaxed) - base + held, n));
        kept_row = kept_row.max(per(LIVE.load(Ordering::Relaxed) - base + held, n));
        assert_eq!(list.len(), n);
    }

    let sizes = [
        ("NodeRow", size_of::<NodeRow>()),
        ("NodeInfo", size_of::<NodeInfo>()),
        ("SerializedType", size_of::<SerializedType>()),
        ("ObjectInfo", size_of::<ObjectInfo>()),
        ("External", size_of::<External>()),
        ("ScriptRef", size_of::<ScriptRef>()),
        ("BlockInfo", size_of::<BlockInfo>()),
        ("DirEntry", size_of::<DirEntry>()),
        ("FileSlot", size_of::<FileSlot>()),
        ("RawStr", size_of::<RawStr>()),
        ("Value", size_of::<Value>()),
        ("(NodeId, Value)", size_of::<(NodeId, Value)>()),
    ];
    for (name, size) in sizes {
        println!("size_of {name:<16} {size:>4} bytes");
    }
    println!(
        "typetree row: {kept_row:.1} bytes kept, {peak_row:.1} at the peak of from_rows (requested bytes)"
    );

    // The largest parsed record, with the side tables kept per record (an
    // object's dict-order index; an entry's dict slot and parse cell).
    let record = [
        size_of::<SerializedType>(),
        size_of::<ObjectInfo>() + 4,
        size_of::<External>(),
        size_of::<ScriptRef>(),
        size_of::<BlockInfo>() + 8,
        size_of::<DirEntry>() + size_of::<FileSlot>() + 64,
    ]
    .into_iter()
    .max()
    .unwrap();
    let l = Limits::default();
    let row = peak_row + size_of::<NodeInfo>() as f64;
    let terms = [
        ("decoded data held", l.max_bundle_uncompressed as f64),
        ("one block's regrowth", l.max_block_output as f64),
        (
            "metadata records",
            l.max_metadata_records as f64 * record as f64,
        ),
        ("typetree rows", l.max_tree_rows as f64 * row),
        (
            "one object's values",
            l.max_values_per_object as f64 * size_of::<(NodeId, Value)>() as f64,
        ),
    ];
    let mut total = 0.0;
    for (what, bytes) in terms {
        println!("{what:<22} {:>6.2} GiB", bytes / GIB);
        total += bytes;
    }
    println!("{:<22} {:>6.2} GiB", "one input, worst case", total / GIB);
    assert!(record <= 200, "a record grew: {record}");
    assert!(
        total / GIB <= 10.0,
        "the default limits no longer fit: {:.2} GiB",
        total / GIB
    );
}
