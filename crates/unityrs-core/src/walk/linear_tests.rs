//! Plan 3.10 and 3.11, linear work: what a hostile tree can make the
//! readers and the writer do again and again for a `ReferencedObject`,
//! counted in map steps ([`STEPS`], never a clock) and bounded linearly in
//! the input (claude X, review round 2: the ref-type scan; claude B, round
//! 2: a dict a repeated name hands many siblings); and the loops no value
//! comes out of, counted in loop turns ([`VISITS`]) and bounded by
//! `Limits::max_values_per_object` (claude B, review round 3: children a
//! read skips, and one value a write writes through many same-named
//! children).

use std::sync::Arc;

use bytes::Bytes;

use super::dict::{STEPS, VISITS};
use crate::budget::DecodeBuf;
use crate::error::{LimitError, LimitKind};
use crate::files::SerializedType;
use crate::limits::{Limits, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{NodeList, NodeRow, Tree};
use crate::value::Parsed;
use crate::walk::{
    ReadCtx, ReadDetail, ReadError, ReadOptions, RefTypes, WriteCtx, WriteError, WriteRules, boost,
    python,
};

/// What one reader's read took, and what writing its value back under one
/// rule took, in map steps.
struct Steps {
    what: String,
    read: u64,
    write: u64,
}

/// Reads `data` through `rows` with both readers, and writes each value
/// back under both rules, counting each in map steps. Every read and write
/// must succeed.
fn measure(rows: Vec<NodeRow>, data: &[u8], refs: &[SerializedType]) -> Vec<Steps> {
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let data = Bytes::copy_from_slice(data);
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    let limits = Limits::default();
    let mut out = Vec::new();
    for reader in ["boost", "python"] {
        STEPS.with(|s| s.set(0));
        let p = match reader {
            "boost" => boost::read(&data, &tree, &ctx),
            _ => python::read(&data, 0, &tree, &ctx),
        }
        .unwrap_or_else(|e| panic!("{reader}: {e}"));
        let read = STEPS.with(std::cell::Cell::get);
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let wctx = WriteCtx {
                endian: Endian::Little,
                ref_types: RefTypes::List(refs),
                rules,
                limits: &limits,
            };
            let mut buf = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
            STEPS.with(|s| s.set(0));
            super::write::write(&p, &tree, &wctx, &mut buf)
                .unwrap_or_else(|e| panic!("{reader} {rules:?}: {e}"));
            out.push(Steps {
                what: format!("{reader} read, {rules:?} write"),
                read,
                write: STEPS.with(std::cell::Cell::get),
            });
        }
    }
    out
}

/// One ref type per class name, each with its own one-row tree (an empty
/// class: its data reads as `{}`).
fn ref_types(classes: impl Iterator<Item = String>) -> Vec<SerializedType> {
    classes
        .map(|c| {
            let rows = vec![NodeRow::new(0, "Empty", "Base")];
            SerializedType::ref_type(
                114,
                RawStr::from(c.as_str()),
                RawStr::from(""),
                RawStr::from(""),
                Some(Arc::new(NodeList::trusted(rows).unwrap())),
            )
        })
        .collect()
}

/// An aligned string: its length, its bytes, zeros to a multiple of 4.
fn string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&i32::try_from(s.len()).unwrap().to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    out.resize(out.len().next_multiple_of(4), 0);
}

/// A `ReferencedType` node at `level` of `extra` ints and then `class`,
/// `ns` and `asm` (so a scan for each passes every int first).
fn type_rows(rows: &mut Vec<NodeRow>, level: i32, extra: u32) {
    rows.push(NodeRow::new(level, "ReferencedType", "type"));
    for i in 0..extra {
        rows.push(NodeRow::new(level + 1, "int", format!("f{i}").as_str()));
    }
    for name in ["class", "ns", "asm"] {
        rows.push(NodeRow::new(level + 1, "string", name));
    }
}

/// A type's bytes: `extra` ints, then the class, an empty `ns` and `asm`.
fn type_data(out: &mut Vec<u8>, extra: u32, class: &str) {
    for i in 0..extra {
        out.extend_from_slice(&i.to_le_bytes());
    }
    string(out, class);
    string(out, "");
    string(out, "");
}

fn check(steps: &[Steps], bound: u64, what: &str) {
    for s in steps {
        assert!(
            s.read <= bound && s.write <= bound,
            "{}: {} read and {} write steps for {what} (bound {bound})",
            s.what,
            s.read,
            s.write
        );
    }
}

/// Claude X's review, round 2: `N` references into `N` ref types, the
/// `i`th naming the `i`th, so UnityPy's scan of the list for each costs
/// `N`²/2; and each reference reads through a tree of its own, so the
/// value's table of node lists has `N` + 1 lists to find each in. Both
/// readers and the writer find them in steps linear in `N`.
#[test]
fn references_into_as_many_ref_types_resolve_in_linear_steps() {
    const N: u32 = 1 << 12;
    let refs = ref_types((0..N).map(|i| format!("C{i}")));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "refs"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "ReferencedObject", "data"),
    ];
    type_rows(&mut rows, 4, 0);
    rows.push(NodeRow::new(4, "ReferencedObjectData", "data"));
    let mut data = N.to_le_bytes().to_vec();
    for i in 0..N {
        type_data(&mut data, 0, &format!("C{i}"));
    }
    let steps = measure(rows, &data, &refs);
    check(&steps, 32 * u64::from(N), "N references into N ref types");
}

/// A `ReferencedObject` of `D` data children read once a `type` of `T`
/// entries, whose `class`, `ns` and `asm` come last: each data child
/// resolved the same `type` again, `D` × 3`T` steps; now once.
#[test]
fn a_type_resolves_once_for_all_its_data_children() {
    const D: u32 = 1 << 12;
    const T: u32 = 1 << 12;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "ReferencedObject", "o"),
    ];
    type_rows(&mut rows, 2, T);
    for i in 0..D {
        rows.push(NodeRow::new(
            2,
            "ReferencedObjectData",
            format!("data{i}").as_str(),
        ));
    }
    let mut data = Vec::new();
    type_data(&mut data, T, "C");
    let steps = measure(rows, &data, &refs);
    check(&steps, 16 * u64::from(D + T), "D data children of one type");
}

/// Claude B's review, round 2, through a `ReferencedObject`: `K`
/// same-named siblings over the last one's dict, whose `type` has `N`
/// entries before `class`, `ns` and `asm`. Each earlier sibling finds its
/// `type`'s children in that `type`, and resolves its data through it; each
/// built the `type`'s name table again, and scanned it three times.
#[test]
fn siblings_share_one_referenced_object_in_linear_steps() {
    const K: u32 = 1 << 12;
    const N: u32 = 1 << 12;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    let mut data = Vec::new();
    for _ in 0..K {
        rows.push(NodeRow::new(1, "ReferencedObject", "a"));
        type_rows(&mut rows, 2, 0);
        rows.push(NodeRow::new(2, "ReferencedObjectData", "data"));
        // An empty class: the data is skipped.
        type_data(&mut data, 0, "");
    }
    rows.push(NodeRow::new(1, "ReferencedObject", "a"));
    type_rows(&mut rows, 2, N);
    rows.push(NodeRow::new(2, "ReferencedObjectData", "data"));
    type_data(&mut data, N, "C");
    let steps = measure(rows, &data, &refs);
    check(&steps, 16 * u64::from(K + N), "K siblings sharing one dict");
}

/// The values bound the work tests cross: small, so a test that the bound
/// stops a loop is quick.
const BOUND: u64 = 1 << 14;

/// The default limits with `max_values_per_object` at [`BOUND`].
fn bounded() -> Limits {
    Limits {
        max_values_per_object: BOUND,
        ..Limits::default()
    }
}

/// The refusal one value past [`BOUND`] is.
const REFUSED: LimitError = LimitError {
    kind: LimitKind::ValuesPerObject,
    value: BOUND + 1,
    max: BOUND,
};

fn compile(rows: Vec<NodeRow>) -> Arc<Tree> {
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// Reads `data` through `tree` with `reader` under `limits`: the outcome and
/// the loop turns it took.
fn read_visits(
    reader: &str,
    tree: &Arc<Tree>,
    data: &[u8],
    refs: &[SerializedType],
    limits: Limits,
) -> (Result<Parsed, ReadError>, u64) {
    let data = Bytes::copy_from_slice(data);
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    VISITS.with(|s| s.set(0));
    let got = match reader {
        "boost" => boost::read(&data, tree, &ctx),
        _ => python::read(&data, 0, tree, &ctx),
    };
    (got, VISITS.with(std::cell::Cell::get))
}

/// Writes `p` back through `tree` under `rules` and `limits`: the outcome
/// and the loop turns it took.
fn write_visits(
    p: &Parsed,
    tree: &Tree,
    refs: &[SerializedType],
    rules: WriteRules,
    limits: &Limits,
) -> (Result<(), WriteError>, u64) {
    let ctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        rules,
        limits,
    };
    let meter = Meter::unlimited();
    let mut buf = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
    VISITS.with(|s| s.set(0));
    let got = super::write::write(p, tree, &ctx, &mut buf);
    (got, VISITS.with(std::cell::Cell::get))
}

/// A read the values bound stopped, in at most twice its turns.
fn assert_read_refused(what: &str, got: &Result<Parsed, ReadError>, visits: u64) {
    match got {
        Err(ReadError {
            detail: ReadDetail::Limit(l),
            ..
        }) => assert_eq!(*l, REFUSED, "{what}"),
        Err(e) => panic!("{what}: {e}, not the values bound"),
        Ok(_) => panic!("{what}: read whole, {visits} loop turns past a bound of {BOUND}"),
    }
    assert!(visits <= 2 * BOUND, "{what}: {visits} loop turns");
}

/// A write the values bound stopped, in at most twice its turns.
fn assert_write_refused(what: &str, got: &Result<(), WriteError>, visits: u64) {
    match got {
        Err(WriteError::Limit(l)) => assert_eq!(*l, REFUSED, "{what}"),
        Err(e) => panic!("{what}: {e}, not the values bound"),
        Ok(()) => panic!("{what}: written whole, {visits} loop turns past a bound of {BOUND}"),
    }
    assert!(visits <= 2 * BOUND, "{what}: {visits} loop turns");
}

/// Claude B's review, round 3, scenario A: `N` `ReferencedObject`s of an
/// empty class, each with `K` data children, all skipped. Each element
/// builds five values and its loop visits `K` + 2 children, so `N` × `K`
/// turns ran under a bound of values that `N` × 5 never reached. Every
/// skipped child now counts one value: both readers stop at the bound, in
/// turns linear in it; and so does a `MirrorReader` write of the value read
/// without the bound, which skips the same children (a `UnityPy` write
/// fails at the first one, W7).
#[test]
fn skipped_data_children_count_against_the_values_bound() {
    const N: u32 = 64;
    const K: u32 = 1024;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "refs"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "ReferencedObject", "data"),
    ];
    type_rows(&mut rows, 4, 0);
    for i in 0..K {
        rows.push(NodeRow::new(
            4,
            "ReferencedObjectData",
            format!("d{i}").as_str(),
        ));
    }
    let tree = compile(rows);
    let mut data = N.to_le_bytes().to_vec();
    for _ in 0..N {
        type_data(&mut data, 0, "");
    }
    assert!(u64::from(N * K) > 2 * BOUND && u64::from(N * 5) < BOUND);
    for reader in ["boost", "python"] {
        let (got, visits) = read_visits(reader, &tree, &data, &refs, bounded());
        assert_read_refused(reader, &got, visits);
        let (p, _) = read_visits(reader, &tree, &data, &refs, Limits::default());
        let p = p.unwrap_or_else(|e| panic!("{reader}, unbounded: {e}"));
        let (got, visits) = write_visits(&p, &tree, &refs, WriteRules::MirrorReader, &bounded());
        assert_write_refused(&format!("{reader} read, MirrorReader write"), &got, visits);
    }
}

/// Claude B's review, round 3, scenario B: `N` classes of an `int` and `K`
/// `ManagedReferencesRegistry` children, of which R13 reads the first and
/// skips the rest. Each skipped one now counts one value, in both readers
/// and in the writer, which skips them too.
#[test]
fn skipped_registries_count_against_the_values_bound() {
    const N: u32 = 64;
    const K: u32 = 1024;
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "v"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "Foo", "data"),
        NodeRow::new(4, "int", "x"),
    ];
    for i in 0..K {
        rows.push(NodeRow::new(
            4,
            "ManagedReferencesRegistry",
            format!("r{i}").as_str(),
        ));
    }
    let tree = compile(rows);
    let mut data = N.to_le_bytes().to_vec();
    for i in 0..N {
        data.extend_from_slice(&i.to_le_bytes());
    }
    assert!(u64::from(N * (K - 1)) > 2 * BOUND && u64::from(N * 4) < BOUND);
    for reader in ["boost", "python"] {
        let (got, visits) = read_visits(reader, &tree, &data, &[], bounded());
        assert_read_refused(reader, &got, visits);
        let (p, _) = read_visits(reader, &tree, &data, &[], Limits::default());
        let p = p.unwrap_or_else(|e| panic!("{reader}, unbounded: {e}"));
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let (got, visits) = write_visits(&p, &tree, &[], rules, &bounded());
            assert_write_refused(&format!("{reader} read, {rules:?} write"), &got, visits);
        }
    }
}

/// Claude B's review, round 3: `Base { a: vector<Empty> × (K - 1), a:
/// vector<int> }`. The read keeps the last vector's `N` ints, and the write
/// sends them through every same-named sibling, as UnityPy's does: an
/// `Empty` element takes any value and writes nothing, so `K` × `N` values
/// were written into `4K + 4N` bytes, with no bound but the output's
/// memory. The writer counts every value it writes under a repeated name
/// against the bound the read counts against; the read passes it
/// (UnityPyBoost's counts the `K` vectors, each run one value; the
/// pure-Python one every value, `K` + `N` + 1). So it does when the
/// siblings are `vector<int>`s, whose elements `MirrorReader` writes as one
/// run.
#[test]
fn one_value_written_through_many_same_named_children_counts_against_the_values_bound() {
    const K: u32 = 64;
    const N: u32 = 4096;
    for first in ["Empty", "int"] {
        let mut rows = vec![NodeRow::new(0, "Base", "Base")];
        for elem in std::iter::repeat_n(first, K as usize - 1).chain(std::iter::once("int")) {
            rows.push(NodeRow::new(1, "vector", "a"));
            rows.push(NodeRow::new(2, "Array", "Array"));
            rows.push(NodeRow::new(3, "int", "size"));
            rows.push(NodeRow::new(3, elem, "data"));
        }
        let tree = compile(rows);
        let mut data = vec![0; 4 * (K as usize - 1)];
        data.extend_from_slice(&N.to_le_bytes());
        for i in 0..N {
            data.extend_from_slice(&i.to_le_bytes());
        }
        assert!(u64::from(K * N) > 2 * BOUND && u64::from(2 * (K + N)) < BOUND);
        for reader in ["boost", "python"] {
            let (p, _) = read_visits(reader, &tree, &data, &[], bounded());
            let p = p.unwrap_or_else(|e| panic!("{first}, {reader}: {e}"));
            for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
                let (got, visits) = write_visits(&p, &tree, &[], rules, &bounded());
                let what = format!("vector<{first}>s, {reader} read, {rules:?} write");
                assert_write_refused(&what, &got, visits);
            }
        }
    }
}

/// The same fan-out through a class that takes a value which is not a dict
/// (W2: each child's attribute): `Base { a: vector<Foo>, a: vector<int> }`,
/// `Foo` a registry named `real` (an `int`'s attribute) then `K` more
/// registries, which R13's scope skips. Each of the `N` ints skipped `K`
/// children uncounted.
#[test]
fn registries_skipped_on_a_value_that_is_not_a_dict_count_too() {
    const N: u32 = 64;
    const K: u32 = 1024;
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "a"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "Foo", "data"),
        NodeRow::new(4, "ManagedReferencesRegistry", "real"),
    ];
    for i in 0..K {
        rows.push(NodeRow::new(
            4,
            "ManagedReferencesRegistry",
            format!("r{i}").as_str(),
        ));
    }
    rows.extend([
        NodeRow::new(1, "vector", "a"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "int", "data"),
    ]);
    let tree = compile(rows);
    let mut data = vec![0; 4];
    data.extend_from_slice(&N.to_le_bytes());
    for i in 0..N {
        data.extend_from_slice(&i.to_le_bytes());
    }
    assert!(u64::from(N * K) > 2 * BOUND && u64::from(2 * N) < BOUND);
    for reader in ["boost", "python"] {
        let (p, _) = read_visits(reader, &tree, &data, &[], bounded());
        let p = p.unwrap_or_else(|e| panic!("{reader}: {e}"));
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let (got, visits) = write_visits(&p, &tree, &[], rules, &bounded());
            assert_write_refused(&format!("{reader} read, {rules:?} write"), &got, visits);
        }
    }
}

/// Review round 9: a value the reader keeps does not count against the
/// bound, but a class written from a value that is not a dict does, since
/// its children are the value's attributes, which the value's size does
/// not bound. `N` ints read as `Base { vector<int> v }` and written
/// through `Base { vector<Foo> v }`, `Foo` four empty classes named for an
/// `int`'s attributes (`real`, `imag`, `numerator`, `denominator`): five
/// classes an element, none writing a byte, and no name repeated.
#[test]
fn classes_written_from_values_that_are_not_dicts_count_against_the_values_bound() {
    const N: u32 = 1 << 12;
    let read_with = compile(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "v"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "int", "data"),
    ]);
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "v"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "Foo", "data"),
    ];
    for name in ["real", "imag", "numerator", "denominator"] {
        rows.push(NodeRow::new(4, "Empty", name));
    }
    let write_with = compile(rows);
    let mut data = N.to_le_bytes().to_vec();
    for i in 0..N {
        data.extend_from_slice(&i.to_le_bytes());
    }
    assert!(u64::from(5 * N) > BOUND && u64::from(5 * N) < 2 * BOUND);
    for reader in ["boost", "python"] {
        let (p, _) = read_visits(reader, &read_with, &data, &[], Limits::default());
        let p = p.unwrap_or_else(|e| panic!("{reader}: {e}"));
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let (got, visits) = write_visits(&p, &write_with, &[], rules, &bounded());
            assert_write_refused(&format!("{reader} read, {rules:?} write"), &got, visits);
        }
    }
}

/// A name a `ReferencedObject`'s children repeat counts as a class's does,
/// in UnityPyBoost's read and in the writer: `ReferencedObject { type; x:
/// vector<string>; x: vector<string>; data }`, the first `x` empty and the
/// second of `N` strings, its class empty (the data skipped). The read
/// keeps the second, and the write writes it through both.
#[test]
fn a_referenced_objects_repeated_names_count_against_the_values_bound() {
    const N: u32 = 1 << 14;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "ReferencedObject", "o"),
    ];
    type_rows(&mut rows, 2, 0);
    for _ in 0..2 {
        rows.extend([
            NodeRow::new(2, "vector", "x"),
            NodeRow::new(3, "Array", "Array"),
            NodeRow::new(4, "int", "size"),
            NodeRow::new(4, "string", "data"),
        ]);
    }
    rows.push(NodeRow::new(2, "ReferencedObjectData", "data"));
    let tree = compile(rows);
    let mut data = Vec::new();
    type_data(&mut data, 0, "");
    data.extend_from_slice(&0_u32.to_le_bytes());
    data.extend_from_slice(&N.to_le_bytes());
    for _ in 0..N {
        string(&mut data, "");
    }
    assert_eq!(u64::from(N), BOUND);
    // UnityPyBoost's read counts the two vectors and the strings, under the
    // repeated name, and the skipped data: `N` + 3.
    let (got, visits) = read_visits("boost", &tree, &data, &refs, bounded());
    assert_read_refused("boost", &got, visits);
    let (p, _) = read_visits("boost", &tree, &data, &refs, Limits::default());
    let p = p.unwrap_or_else(|e| panic!("boost, unbounded: {e}"));
    // The write sends the second vector through both children, `N` + 1
    // values each, so it passes the bound in the first.
    for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
        let (got, visits) = write_visits(&p, &tree, &refs, rules, &bounded());
        assert_write_refused(&format!("boost read, {rules:?} write"), &got, visits);
    }
}

/// Review round 7's sweep: a list read past its first reservation grew by
/// exact steps of `max_prealloc` elements, so one of `n` moved `n` /
/// `max_prealloc` times, each move copying every element read so far: `n`²
/// / `max_prealloc` copies, none of them counted. It now grows
/// geometrically, never past its length: 2^20 elements past a first
/// reservation of 2^16 move 4 times (2^17, 2^18, 2^19, 2^20), and end with
/// no spare capacity.
#[test]
fn a_list_past_its_first_reservation_grows_in_logarithmic_moves() {
    const LEN: usize = 1 << 20;
    let meter = Meter::unlimited();
    let mut items: MeteredVec<u8> = MeteredVec::new(&meter);
    let first = super::first_reserve(LEN, usize::MAX, 1, Limits::default().max_prealloc);
    items.reserve_exact(first).unwrap();
    let mut moves = 0;
    for _ in 0..LEN {
        let capacity = items.capacity();
        super::grow_step(&mut items, LEN).unwrap();
        moves += u32::from(items.capacity() != capacity);
        items.push(0).unwrap();
    }
    assert_eq!(items.capacity(), LEN);
    assert_eq!(moves, 4);
}
