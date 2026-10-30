//! Package B's memory test (plan sections 3.15 and 7.3): every allocation a
//! typetree read or a write makes is charged to the input's meter before it
//! is made.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_memory
//!
//! A counting global allocator, the one `memory_model.rs` uses (the heap
//! counted in glibc chunks, [`heap_cost`] of each request, a moving
//! `realloc` counted as both blocks at once), checks at **every
//! allocation** of a read or a write that the heap above the run's start is
//! at most what the meter has charged at that moment: no slack. It runs
//! every recorded case through both readers and, after each successful
//! read, through the writer under both rules, then a class of 2^16 + 1
//! distinct children, a list of 2^16 + 1 strings, two classes of one name
//! over 2^16 + 1 children (the writer's name table), 4,096 references into
//! as many ref types (the ref-type and node-list tables) and two classes of
//! one name over 4,096 dicts (the writer's kept name tables), each with the
//! meter booking an `Unbounded` budget and a `Counting` one. The largest request
//! of each big read is the exact size of its 2^16 + 1 elements: the dict's
//! entries and the list's items end with their capacity equal to their
//! length (plan 3.10, "list growth"). The allocator counts only the test's
//! own thread (`unityrs_testkit::counted`): libtest's main thread allocates
//! beside it.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value as Json;
use unityrs_core::budget::{
    Counting as CountingBudget, DecodeBuf, MemoryBudget, Reservation, Unbounded,
};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter, heap_cost};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadError, ReadOptions, RefTypes, WriteCtx, WriteRules};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr, Value};
use unityrs_testkit::counted::{count_this_thread, counted, others_are_not_counted};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The heap when the watched run started.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The most the heap above `BASE` exceeded the watched meter's charge.
static EXCESS: AtomicUsize = AtomicUsize::new(0);
/// The largest single request while watched.
static MAX_REQUEST: AtomicUsize = AtomicUsize::new(0);
std::thread_local! {
    /// The meter being watched, or null: the test thread's, the only thread
    /// counted.
    static WATCHED: Cell<*const Meter> = const { Cell::new(ptr::null()) };
}

/// The glibc chunk a request of `n` bytes takes.
fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

/// Called with the heap just after an allocation is counted.
fn check(now: usize, request: usize) {
    let m = WATCHED.try_with(Cell::get).unwrap_or(ptr::null());
    if m.is_null() {
        return;
    }
    // SAFETY: `watch` stores, on this thread, a pointer to a meter its caller
    // keeps alive until `unwatch` clears it on this thread; `used` reads an
    // atomic and allocates nothing.
    let used = usize::try_from(unsafe { &*m }.used()).unwrap_or(usize::MAX);
    let over = now
        .saturating_sub(BASE.load(Ordering::Relaxed))
        .saturating_sub(used);
    EXCESS.fetch_max(over, Ordering::Relaxed);
    MAX_REQUEST.fetch_max(request, Ordering::Relaxed);
}

// SAFETY: every method forwards to `System` and only counts sizes, on the
// thread under test.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counted() {
            let c = chunk(layout.size());
            let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
            check(now, layout.size());
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
        // Count the worst case of a moving realloc: both blocks live at once.
        if on {
            let c = chunk(new_size);
            let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
            check(now, new_size);
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

/// Starts watching `m` (whose caller keeps it alive until [`unwatch`]).
fn watch(m: &Arc<Meter>) {
    assert_eq!(m.used(), 0);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    EXCESS.store(0, Ordering::Relaxed);
    MAX_REQUEST.store(0, Ordering::Relaxed);
    WATCHED.with(|w| w.set(Arc::as_ptr(m)));
}

/// Stops watching; the excess seen, which must be zero.
fn unwatch(what: &str) {
    WATCHED.with(|w| w.set(ptr::null()));
    let excess = EXCESS.load(Ordering::Relaxed);
    assert_eq!(
        excess, 0,
        "{what}: the heap went {excess} bytes above the charge"
    );
}

/// The heap above the start is at most the charge, now.
fn held_within_charge(m: &Meter, what: &str) {
    let live = LIVE
        .load(Ordering::Relaxed)
        .saturating_sub(BASE.load(Ordering::Relaxed));
    let used = usize::try_from(m.used()).unwrap();
    assert!(live <= used, "{what}: heap {live} above the charge {used}");
}

fn meter(counting: bool) -> Arc<Meter> {
    let budget: Arc<dyn MemoryBudget> = if counting {
        Arc::new(CountingBudget::new(u64::MAX))
    } else {
        Arc::new(Unbounded)
    };
    Meter::new(u64::MAX, budget, Reservation::Input)
}

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/walk/");

fn load(name: &str) -> Json {
    let text = std::fs::read_to_string(format!("{DIR}{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A node list whose names are already shared, so cloning one into an
/// error allocates nothing (a `Bytes` made from a `Vec` allocates its
/// shared header on its first clone).
fn nodes(rows: &Json) -> Arc<NodeList> {
    let rows: Vec<NodeRow> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
                .with_meta_flag(i32::try_from(r[3].as_i64().unwrap()).unwrap())
        })
        .collect();
    let list = NodeList::trusted(rows).unwrap();
    for r in list.rows() {
        drop((r.ty.clone(), r.name.clone()));
    }
    Arc::new(list)
}

fn endian(c: &Json) -> Endian {
    match c.get("endian").and_then(Json::as_str) {
        Some(">") => Endian::Big,
        _ => Endian::Little,
    }
}

/// A case's ref types, their trees compiled before anything is watched
/// (a tree is charged to its own list's meter, not the read's).
fn ref_types(spec: Option<&Json>) -> Option<Vec<SerializedType>> {
    let list = match spec {
        None => Vec::new(),
        Some(Json::Array(a)) => a
            .iter()
            .map(|r| {
                let tree = (!r["nodes"].is_null()).then(|| nodes(&r["nodes"]));
                SerializedType::ref_type(
                    114,
                    RawStr::from(r["class"].as_str().unwrap()),
                    RawStr::from(r["ns"].as_str().unwrap()),
                    RawStr::from(r["asm"].as_str().unwrap()),
                    tree,
                )
            })
            .collect(),
        Some(_) => return None,
    };
    for t in &list {
        let _ = t.tree(&Limits::default());
    }
    Some(list)
}

/// Reads `object` (or `file` from 0, for the pure-Python reader) under
/// watch, then writes what it read under both rules, then drops it all:
/// every allocation charged, and everything returned.
#[allow(clippy::too_many_arguments)]
fn measure(
    what: &str,
    python: bool,
    file: &Bytes,
    object: &Bytes,
    tree: &Arc<Tree>,
    refs: RefTypes<'_>,
    endian: Endian,
    counting: bool,
) -> (Option<usize>, bool) {
    let m = meter(counting);
    let options = ReadOptions::default();
    let limits = Limits::default();
    let ctx = ReadCtx {
        endian,
        ref_types: refs,
        options: &options,
        meter: &m,
    };
    watch(&m);
    let got: Result<Parsed, ReadError> = if python {
        walk::python::read(file, 0, tree, &ctx)
    } else {
        walk::boost::read(object, tree, &ctx)
    };
    let largest = MAX_REQUEST.load(Ordering::Relaxed);
    held_within_charge(&m, what);
    assert_eq!(
        EXCESS.load(Ordering::Relaxed),
        0,
        "{what}: the read went above the charge"
    );
    let ok = got.is_ok();
    if let Ok(p) = &got {
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let wctx = WriteCtx {
                endian,
                ref_types: refs,
                rules,
                limits: &limits,
            };
            let mut out = DecodeBuf::new(&m, 0, u64::MAX, LimitKind::InputMemory).unwrap();
            let _ = walk::write::write(p, tree, &wctx, &mut out);
            held_within_charge(&m, what);
            assert_eq!(
                EXCESS.load(Ordering::Relaxed),
                0,
                "{what}: the {rules:?} write went above the charge"
            );
            drop(out);
        }
    }
    drop(got);
    unwatch(what);
    assert_eq!(m.used(), 0, "{what}: everything returned");
    (ok.then_some(largest), ok)
}

fn cases(counting: bool) -> usize {
    let cases = load("cases.json");
    let mut ok = 0;
    for c in cases.as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let tree = Tree::compile(nodes(&c["nodes"]), &Limits::default()).unwrap();
        let refs = ref_types(c.get("ref_types"));
        let refs = match (&refs, c.get("ref_types")) {
            (Some(l), _) => RefTypes::List(l),
            (None, Some(Json::Null)) => RefTypes::NoFile,
            (None, _) => RefTypes::Absent,
        };
        let file = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        let start = usize::try_from(c.get("start").and_then(Json::as_i64).unwrap_or(0)).unwrap();
        // Shared before watching: a slice of a `Bytes` made from a `Vec`
        // allocates the shared header once.
        let object = file.slice(start..);
        let file = if start == 0 {
            file
        } else {
            // The pure-Python reader reads from 0 here; its own offset rules
            // are the fixtures' business.
            object.clone()
        };
        for python in [false, true] {
            let (_, read) = measure(
                name,
                python,
                &file,
                &object,
                &tree,
                refs,
                endian(c),
                counting,
            );
            ok += usize::from(read);
        }
    }
    ok
}

/// A class of `n` distinct `int` children, and a list of `n` strings.
fn big(n: usize, counting: bool) {
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    rows.extend((0..n).map(|i| NodeRow::new(1, "int", format!("n{i}").as_str())));
    let class = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let class_data = Bytes::from(
        (0..n)
            .flat_map(|i| u32::try_from(i).unwrap().to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let _shared = class_data.clone();
    let list = Tree::compile(
        Arc::new(
            NodeList::trusted(vec![
                NodeRow::new(0, "Base", "Base"),
                NodeRow::new(1, "vector", "v"),
                NodeRow::new(2, "Array", "Array"),
                NodeRow::new(3, "int", "size"),
                NodeRow::new(3, "string", "data"),
            ])
            .unwrap(),
        ),
        &Limits::default(),
    )
    .unwrap();
    let mut list_data = u32::try_from(n).unwrap().to_le_bytes().to_vec();
    list_data.resize(4 + 4 * n, 0);
    let list_data = Bytes::from(list_data);
    let _shared = list_data.clone();
    for python in [false, true] {
        let what = if python {
            "class [python]"
        } else {
            "class [boost]"
        };
        let (largest, _) = measure(
            what,
            python,
            &class_data,
            &class_data,
            &class,
            RefTypes::List(&[]),
            Endian::Little,
            counting,
        );
        assert_eq!(
            largest,
            Some(n * size_of::<(NodeId, Value)>()),
            "{what}: the dict's entries are its largest request, exactly {n} long"
        );
        let what = if python {
            "list [python]"
        } else {
            "list [boost]"
        };
        let (largest, _) = measure(
            what,
            python,
            &list_data,
            &list_data,
            &list,
            RefTypes::List(&[]),
            Endian::Little,
            counting,
        );
        assert_eq!(
            largest,
            Some(n * size_of::<Value>()),
            "{what}: the list's items are its largest request, exactly {n} long"
        );
    }
}

/// Two classes of one name over the same `n` names, the second's in the
/// reverse order: the dict both are written from is keyed by the second's
/// children, so the first finds each entry in the writer's name table, built
/// on its first miss.
fn renamed(n: usize, counting: bool) {
    let mut rows = vec![NodeRow::new(0, "Base", "Base"), NodeRow::new(1, "Foo", "a")];
    rows.extend((0..n).map(|i| NodeRow::new(2, "int", format!("n{i}").as_str())));
    rows.push(NodeRow::new(1, "Bar", "a"));
    rows.extend(
        (0..n)
            .rev()
            .map(|i| NodeRow::new(2, "int", format!("n{i}").as_str())),
    );
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let data = Bytes::from(
        (0..2 * n)
            .flat_map(|i| u32::try_from(i).unwrap().to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let _shared = data.clone();
    for python in [false, true] {
        let what = if python {
            "renamed class [python]"
        } else {
            "renamed class [boost]"
        };
        let (_, read) = measure(
            what,
            python,
            &data,
            &data,
            &tree,
            RefTypes::List(&[]),
            Endian::Little,
            counting,
        );
        assert!(read, "{what}: read");
    }
}

/// An aligned string: its length, its bytes, zeros to a multiple of 4.
fn string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&i32::try_from(s.len()).unwrap().to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    out.resize(out.len().next_multiple_of(4), 0);
}

/// `n` references into `n` ref types, each with a tree of its own: the
/// read's and the write's ref-type table (built once their scans pass the
/// list) and the value's table of node lists (past eight lists).
fn references(n: u32, counting: bool) {
    let refs: Vec<SerializedType> = (0..n)
        .map(|i| {
            SerializedType::ref_type(
                114,
                RawStr::from(format!("C{i}").as_str()),
                RawStr::from(""),
                RawStr::from(""),
                Some(Arc::new(
                    NodeList::trusted(vec![NodeRow::new(0, "Empty", "Base")]).unwrap(),
                )),
            )
        })
        .collect();
    for t in &refs {
        let _ = t.tree(&Limits::default());
    }
    let rows = [
        (0, "Base", "Base"),
        (1, "vector", "refs"),
        (2, "Array", "Array"),
        (3, "int", "size"),
        (3, "ReferencedObject", "data"),
        (4, "ReferencedType", "type"),
        (5, "string", "class"),
        (5, "string", "ns"),
        (5, "string", "asm"),
        (4, "ReferencedObjectData", "data"),
    ];
    let tree = Tree::compile(
        Arc::new(
            NodeList::trusted(
                rows.iter()
                    .map(|(l, t, n)| NodeRow::new(*l, *t, *n))
                    .collect(),
            )
            .unwrap(),
        ),
        &Limits::default(),
    )
    .unwrap();
    let mut data = n.to_le_bytes().to_vec();
    for i in 0..n {
        string(&mut data, &format!("C{i}"));
        string(&mut data, "");
        string(&mut data, "");
    }
    let data = Bytes::from(data);
    let _shared = data.clone();
    for python in [false, true] {
        let what = if python {
            "references [python]"
        } else {
            "references [boost]"
        };
        let (_, read) = measure(
            what,
            python,
            &data,
            &data,
            &tree,
            RefTypes::List(&refs),
            Endian::Little,
            counting,
        );
        assert!(read, "{what}: read");
    }
}

/// Two classes of one name, each over a list of `n` dicts, the second's
/// with a field first that the first's lacks: the dict both are written
/// from is the second's, whose `n` dicts each miss the first's cursor, so
/// the writer keeps `n` name tables in its map of them.
fn shared(n: u32, counting: bool) {
    let rows = [
        (0, "Base", "Base"),
        (1, "Foo", "a"),
        (2, "vector", "v"),
        (3, "Array", "Array"),
        (4, "int", "size"),
        (4, "X", "data"),
        (5, "int", "zz"),
        (1, "Bar", "a"),
        (2, "vector", "v"),
        (3, "Array", "Array"),
        (4, "int", "size"),
        (4, "Y", "data"),
        (5, "int", "c0"),
        (5, "int", "zz"),
    ];
    let tree = Tree::compile(
        Arc::new(
            NodeList::trusted(
                rows.iter()
                    .map(|(l, t, n)| NodeRow::new(*l, *t, *n))
                    .collect(),
            )
            .unwrap(),
        ),
        &Limits::default(),
    )
    .unwrap();
    let mut data = 0_u32.to_le_bytes().to_vec();
    data.extend_from_slice(&n.to_le_bytes());
    data.extend((0..2 * n).flat_map(u32::to_le_bytes));
    let data = Bytes::from(data);
    let _shared = data.clone();
    for python in [false, true] {
        let what = if python {
            "shared dicts [python]"
        } else {
            "shared dicts [boost]"
        };
        let (_, read) = measure(
            what,
            python,
            &data,
            &data,
            &tree,
            RefTypes::List(&[]),
            Endian::Little,
            counting,
        );
        assert!(read, "{what}: read");
    }
}

#[test]
fn every_allocation_of_a_read_and_a_write_is_charged() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    for counting in [false, true] {
        let ok = cases(counting);
        assert!(ok > 200, "only {ok} reads succeeded");
        big((1 << 16) + 1, counting);
        renamed((1 << 16) + 1, counting);
        references(1 << 12, counting);
        shared(1 << 12, counting);
    }
}
