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
//! meter booking an `Unbounded` budget and a `Counting` one. It also watches
//! the typetree compile on the list's own meter ([`compiles`]): a
//! `Tree::compile`, and a `NameTable::new` with a `Tree::compile_with_names`
//! through it, whose rows build every scratch table the compile makes, and
//! the tables the rows come through ([`parses`]): a blob's and an old-format
//! tree's parse with `NodeList::from_rows` after it (review round 15: no
//! counting test saw this scratch). The largest request
//! of each big read is the exact size of its 2^16 + 1 elements: the dict's
//! entries and the list's items end with their capacity equal to their
//! length (plan 3.10, "list growth"). The allocator counts only the test's
//! own thread (`unityrs_testkit::counted`): libtest's main thread allocates
//! beside it.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value as Json;
use unityrs_core::budget::{
    Counting as CountingBudget, DecodeBuf, MemoryBudget, Reservation, Unbounded,
};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::stream::{Endian, Reader};
use unityrs_core::typetree::parse::{parse_blob, parse_old};
use unityrs_core::typetree::tree::NameTable;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadError, ReadOptions, RefTypes, WriteCtx, WriteRules};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr, Value};
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree, old_tree};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The heap when the watched run started.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The watched meter's charge when the run started ([`watch_from`]).
static START: AtomicUsize = AtomicUsize::new(0);
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
    // SAFETY: `watch` stores, on this thread, a pointer to a meter that the
    // `Watch` it returns borrows, and the `Watch` clears it on this thread
    // when it drops, a panic's unwind included; `used` reads an atomic and
    // allocates nothing.
    let used = usize::try_from(unsafe { &*m }.used()).unwrap_or(usize::MAX);
    let over = now
        .saturating_sub(BASE.load(Ordering::Relaxed))
        .saturating_sub(used.saturating_sub(START.load(Ordering::Relaxed)));
    EXCESS.fetch_max(over, Ordering::Relaxed);
    MAX_REQUEST.fetch_max(request, Ordering::Relaxed);
}

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        let c = chunk(size);
        let now = LIVE.fetch_add(c, Ordering::Relaxed) + c;
        check(now, size);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// A watch of a meter on this thread, which ends when it drops, a panic's
/// unwind included; it borrows the meter, so the meter outlives it and the
/// allocator never reads a freed one (review round 6's sweep: a failing
/// check between `watch` and `unwatch` left the pointer set, and the test
/// thread's later allocations read the meter its unwind had freed).
struct Watch<'m>(std::marker::PhantomData<&'m Meter>);

impl Drop for Watch<'_> {
    fn drop(&mut self) {
        let _ = WATCHED.try_with(|w| w.set(ptr::null()));
    }
}

/// Starts watching `m` until the watch drops ([`unwatch`]).
fn watch(m: &Arc<Meter>) -> Watch<'_> {
    assert_eq!(m.used(), 0);
    watch_from(m)
}

/// Starts watching `m`, which may hold charges already (a node list's):
/// the heap may grow by what the meter's charge grows by.
fn watch_from(m: &Arc<Meter>) -> Watch<'_> {
    START.store(usize::try_from(m.used()).unwrap(), Ordering::Relaxed);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    EXCESS.store(0, Ordering::Relaxed);
    MAX_REQUEST.store(0, Ordering::Relaxed);
    WATCHED.with(|w| w.set(Arc::as_ptr(m)));
    Watch(std::marker::PhantomData)
}

/// Stops watching; the excess seen, which must be zero.
fn unwatch(watching: Watch<'_>, what: &str) {
    drop(watching);
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
    let watching = watch(&m);
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
    unwatch(watching, what);
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
    // A panic inside a watch ends it ([`Watch`]): the pointer is cleared
    // before the unwind frees the meter.
    let unwound = std::panic::catch_unwind(|| {
        let m = meter(false);
        let _watching = watch(&m);
        panic!("inside a watch, on purpose");
    });
    assert!(unwound.is_err());
    assert!(
        WATCHED.with(Cell::get).is_null(),
        "the meter's pointer outlived the watch"
    );
    a_watch_sees_what_no_charge_covers();
    for counting in [false, true] {
        let ok = cases(counting);
        assert!(ok > 200, "only {ok} reads succeeded");
        big((1 << 16) + 1, counting);
        renamed((1 << 16) + 1, counting);
        references(1 << 12, counting);
        shared(1 << 12, counting);
        compiles(counting);
        parses(counting);
    }
}

/// The watch's own check, on a meter that holds a charge when the watch
/// starts ([`watch_from`]): an allocation no charge covers shows in full,
/// however much less it is than what the meter already held.
fn a_watch_sees_what_no_charge_covers() {
    let m = meter(false);
    let held = MeteredVec::<u8>::with_capacity(&m, 1 << 12).unwrap();
    let watching = watch_from(&m);
    let plain = std::hint::black_box(vec![0_u8; 64]);
    let excess = EXCESS.load(Ordering::Relaxed);
    drop(watching);
    drop((plain, held));
    assert_eq!(
        excess,
        chunk(64),
        "the watch missed an uncharged allocation"
    );
}

/// Review round 15's sweep: the tables a tree's rows come through before any
/// compile, each charged before it is allocated. A comb 64 levels deep (two
/// children a level, the first going on, so a stack of open parents grows),
/// as a blob (the row table, the string buffer's NULs, an unknown common
/// offset's decimal string) and in the old format (the row table, its stack),
/// each then through `NodeList::from_rows` (its stack, `parents`,
/// `child_start`, `cursor`, `children`).
fn parses(counting: bool) {
    let mut nodes = vec![NodeSpec::new(0, "Base", "Base")];
    nodes.extend((1..=64).map(|level| NodeSpec::new(level, "int", "a")));
    nodes.extend((1..=64).rev().map(|level| NodeSpec::new(level, "int", "b")));
    let common: [(u32, &[u8]); 1] = [(99_999, b"int")];
    for blob in [true, false] {
        let data = Bytes::from(if blob {
            blob_tree(&nodes, 19, false, Strings::Common, &common)
        } else {
            old_tree(&nodes, 9, false)
        });
        // Its shared header, which the rows' slices would allocate once.
        drop(data.clone());
        let m = meter(counting);
        let limits = Limits::default();
        let mut r = Reader::new(&data, Endian::Little);
        let watching = watch(&m);
        let rows = if blob {
            parse_blob(&mut r, &data, 19, &m, &limits, None)
        } else {
            parse_old(&mut r, &data, 9, &m, &limits)
        };
        let list = NodeList::from_rows(rows.unwrap()).unwrap();
        unwatch(watching, &format!("a parse, a blob: {blob}"));
        assert_eq!(list.len(), nodes.len());
        let ty = list.rows()[1].ty.as_bytes();
        assert_eq!(ty, if blob { &b"99999"[..] } else { &b"int"[..] });
    }
}

/// FNV-1a, the compile's name hash (`typetree/tree/mod.rs`, `name_hash`).
fn fnv1a(name: &[u8]) -> u64 {
    name.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Two names of 8 bytes and one FNV-1a hash (found by a distinguished-point
/// search): a run of one hash holding both is sorted (`names.rs`, `tmp`).
const A: [u8; 8] = 0x037a_85c7_f7a9_042c_u64.to_le_bytes();
const B: [u8; 8] = 0xe69f_154c_6166_a4a8_u64.to_le_bytes();
/// An 8-byte name of a hash of its own.
const C: [u8; 8] = *b"cccccccc";

/// The rows `compiles` compiles, each name a string of its own (`fresh`) or
/// a table's (`theirs`): 256 names of 5 bytes (the hashes), 64 copies of one
/// name (a run of one hash), 64 of `A` then 64 of `other` (with `B`, a run
/// of two names: the sort), a vector (depths) and a type whose head holds a
/// NUL (the pure-Python kinds).
fn compile_rows(other: [u8; 8], name: &mut impl FnMut(&[u8]) -> RawStr) -> Vec<NodeRow> {
    let mut rows = vec![NodeRow::new(0, "Base", name(b"Base"))];
    let mut add = |level, ty: &str, n: &[u8]| rows.push(NodeRow::new(level, ty, name(n)));
    (0..256).for_each(|i| add(1, "int", format!("n{i:04}").as_bytes()));
    (0..64).for_each(|_| add(1, "int", b"dup"));
    (0..64).for_each(|_| add(1, "int", &A));
    (0..64).for_each(|_| add(1, "int", &other));
    for (level, ty, n) in [
        (1, "vector", "v"),
        (2, "Array", "Array"),
        (3, "int", "size"),
    ] {
        add(level, ty, n.as_bytes());
    }
    add(3, "int", b"data");
    add(1, "in\0t", b"z");
    rows
}

/// A compile of `rows` on `m`'s meter, under watch when `watched`; the work
/// it counted.
fn compile_on(m: &Arc<Meter>, rows: Vec<NodeRow>, watched: bool, names: Option<&NameTable>) -> u64 {
    let list = Arc::new(NodeList::from_rows(MeteredVec::from_vec(rows, m).unwrap()).unwrap());
    let (work, watching) = (m.decoded(), watched.then(|| watch_from(m)));
    let tree = match names {
        Some(t) => Tree::compile_with_names(list, &Limits::default(), t),
        None => Tree::compile(list, &Limits::default()),
    };
    let work = m.decoded() - work;
    drop(tree.unwrap());
    if let Some(watching) = watching {
        unwatch(
            watching,
            &format!("a compile, names from a table: {}", names.is_some()),
        );
    }
    work
}

/// Review round 15: the compile's scratch tables (the depths, the names'
/// order, ids, buffers, hashes, runs and sort, the classes' `seen`, the
/// table's ids and their sorted copy) and the tables it keeps (`info`, the
/// pure-Python kinds, the tree's `Arc`) are each charged to the list's meter
/// before they are allocated, through a plain compile and through a
/// `NameTable` as a TPK's trees are compiled (its strings slices of one
/// buffer, the rows' names clones of them).
fn compiles(counting: bool) {
    let mut fresh = |n: &[u8]| RawStr::from(n);
    let m = meter(counting);
    assert_eq!(fnv1a(&A), fnv1a(&B));
    let sorted = compile_on(&m, compile_rows(B, &mut fresh), true, None);
    // Only a collision sorts (`tmp`): with `C` for `B` no run is sorted, and
    // the comparisons of neighbours after the sort alone are 127 of 8 bytes.
    let unsorted = compile_on(&meter(counting), compile_rows(C, &mut fresh), false, None);
    assert!(
        sorted >= unsorted + 127 * 8,
        "A and B were not sorted: {sorted} {unsorted}"
    );

    let mut text: Vec<u8> = Vec::new();
    let mut at = Vec::new();
    for row in compile_rows(B, &mut fresh) {
        for s in [row.ty.as_bytes(), row.name.as_bytes()] {
            at.push(text.len()..text.len() + s.len());
            text.extend_from_slice(s);
        }
    }
    let text = Bytes::from(text);
    let strings: Vec<RawStr> = at
        .iter()
        .map(|r| RawStr::from(text.slice(r.clone())))
        .collect();
    let m = meter(counting);
    let watching = watch_from(&m);
    let table = NameTable::new(&strings, &m).unwrap();
    unwatch(watching, "a name table");
    let mut next = strings.iter().skip(1).step_by(2);
    let mut theirs = |n: &[u8]| {
        let s = next.next().unwrap().clone();
        assert_eq!(s.as_bytes(), n);
        s
    };
    let work = compile_on(&m, compile_rows(B, &mut theirs), true, Some(&table));
    assert_eq!(work, 0, "the compile through the table read no name");
}
