//! Package B's walker tests beyond the oracle's cases (plan section 7.3):
//! the value's size, a meter too small, both float modes, ref types read
//! through one another, each limit, a repeated name after a skipped
//! registry, list capacity, and seeded no-panic runs over random trees and
//! bytes.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_rules

use std::sync::Arc;

use unityrs_core::budget::{DecodeBuf, Reservation, Unbounded};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree, TreeError};
use unityrs_core::walk::{
    self, FloatMode, ReadCtx, ReadDetail, ReadError, ReadErrorKind, ReadOptions, RefTypes,
    WriteCtx, WriteError, WriteRules,
};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr, Value};
use unityrs_testkit::rng::Rng;
use unityrs_testkit::writer::ByteWriter;

const ALIGN: i32 = 0x4000;

type Row<'a> = (i32, &'a str, &'a str, i32);

fn list(rows: &[Row<'_>]) -> Arc<NodeList> {
    let rows = rows
        .iter()
        .map(|&(level, ty, name, meta)| NodeRow::new(level, ty, name).with_meta_flag(meta))
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

fn tree(rows: &[Row<'_>]) -> Arc<Tree> {
    Tree::compile(list(rows), &Limits::default()).unwrap()
}

/// A `ReferencedObject` named `name` at `level`: its `type` and its `data`.
fn ref_rows(level: i32, name: &str) -> Vec<Row<'_>> {
    vec![
        (level, "ReferencedObject", name, 0),
        (level + 1, "ReferencedManagedType", "type", 0),
        (level + 2, "string", "class", 0),
        (level + 2, "string", "ns", 0),
        (level + 2, "string", "asm", 0),
        (level + 1, "ReferencedObjectData", "data", 0),
    ]
}

fn ref_type(class: &str, rows: &[Row<'_>]) -> SerializedType {
    SerializedType::ref_type(
        114,
        RawStr::from(class),
        RawStr::from("n"),
        RawStr::from("a"),
        Some(list(rows)),
    )
}

/// An aligned string, as `write_aligned_string` writes it.
fn string(w: &mut ByteWriter, s: &str) {
    w.i32(i32::try_from(s.len()).unwrap())
        .bytes(s.as_bytes())
        .align(4);
}

/// Reads `data` with both readers (UnityPyBoost's, from offset 0, and the
/// pure-Python one).
fn both(
    data: &[u8],
    tree: &Arc<Tree>,
    refs: RefTypes<'_>,
    options: &ReadOptions,
    meter: &Arc<Meter>,
) -> [(&'static str, Result<Parsed, ReadError>); 2] {
    let data = Bytes::copy_from_slice(data);
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: refs,
        options,
        meter,
    };
    [
        ("boost", walk::boost::read(&data, tree, &ctx)),
        ("python", walk::python::read(&data, 0, tree, &ctx)),
    ]
}

fn read_both(data: &[u8], tree: &Arc<Tree>, refs: RefTypes<'_>) -> [(&'static str, Parsed); 2] {
    let options = ReadOptions::default();
    both(data, tree, refs, &options, &Meter::unlimited())
        .map(|(reader, r)| (reader, r.unwrap_or_else(|e| panic!("{reader}: {e:?}"))))
}

fn write(
    p: &Parsed,
    tree: &Tree,
    refs: RefTypes<'_>,
    rules: WriteRules,
    limits: &Limits,
) -> Result<Vec<u8>, WriteError> {
    let ctx = WriteCtx {
        endian: Endian::Little,
        ref_types: refs,
        rules,
        limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    walk::write::write(p, tree, &ctx, &mut out)?;
    Ok(out.as_slice().to_vec())
}

fn assert_limit(r: &Result<Parsed, ReadError>, kind: LimitKind, what: &str) {
    match r {
        Err(ReadError {
            kind: ReadErrorKind::Other,
            detail: ReadDetail::Limit(l),
            ..
        }) if l.kind == kind => {}
        other => panic!("{what}: want an Other {kind:?} limit, have {other:?}"),
    }
}

/// Plan 3.9: a value is at most 40 bytes.
#[test]
fn a_value_is_at_most_40_bytes() {
    assert!(size_of::<Value>() <= 40, "{}", size_of::<Value>());
}

/// Plan 3.15: a meter too small for a value refuses it with
/// `ReadDetail::Charge`, from a run (`Prims`) and from a list of strings,
/// and returns everything the read had charged.
#[test]
fn a_meter_too_small_refuses_the_value() {
    let ints = tree(&[
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "int", "data", 0),
    ]);
    let mut w = ByteWriter::le();
    w.i32(4096);
    for i in 0..4096 {
        w.i32(i);
    }
    let int_data = w.into_inner();
    let strings = tree(&[
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "string", "data", 0),
    ]);
    let mut w = ByteWriter::le();
    w.i32(1024);
    for _ in 0..1024 {
        string(&mut w, "ab");
    }
    let string_data = w.into_inner();
    let options = ReadOptions::default();
    for (what, t, data) in [
        ("ints", &ints, &int_data),
        ("strings", &strings, &string_data),
    ] {
        let small = Meter::new(4096, Arc::new(Unbounded), Reservation::Input);
        for (reader, r) in both(data, t, RefTypes::List(&[]), &options, &small) {
            assert!(
                matches!(
                    &r,
                    Err(ReadError {
                        detail: ReadDetail::Charge(_),
                        ..
                    })
                ),
                "{what} [{reader}]: {r:?}"
            );
            assert_eq!(small.used(), 0, "{what} [{reader}]: the charge is returned");
        }
        for (reader, r) in both(data, t, RefTypes::List(&[]), &options, &Meter::unlimited()) {
            assert!(
                r.is_ok(),
                "{what} [{reader}] reads on an unlimited meter: {r:?}"
            );
        }
    }
}

/// `FloatMode`: UnityPy's widening quiets a signalling `float` NaN (bit
/// 22 set) and keeps a `double`'s bits; `Lossless` keeps both, and the
/// writer writes each value's bits back.
#[test]
fn float_modes_quiet_or_keep_a_signalling_nan() {
    let t = tree(&[
        (0, "Base", "Base", 0),
        (1, "float", "f", 0),
        (1, "double", "d", 0),
    ]);
    let mut w = ByteWriter::le();
    w.u32(0x7f80_0001).u64(0x7ff0_0000_0000_0001);
    let data = w.into_inner();
    for (floats, f) in [
        (FloatMode::UnityPy, 0x7fc0_0001_u32),
        (FloatMode::Lossless, 0x7f80_0001),
    ] {
        let options = ReadOptions {
            floats,
            ..ReadOptions::default()
        };
        for (reader, r) in both(
            &data,
            &t,
            RefTypes::List(&[]),
            &options,
            &Meter::unlimited(),
        ) {
            let p = r.unwrap();
            assert_eq!(
                p.get(&[b"f"]),
                Some(&Value::F32(f)),
                "{floats:?} [{reader}]"
            );
            assert_eq!(
                p.get(&[b"d"]),
                Some(&Value::F64(0x7ff0_0000_0000_0001)),
                "{floats:?} [{reader}]"
            );
            let out = write(
                &p,
                &t,
                RefTypes::List(&[]),
                WriteRules::UnityPy,
                &Limits::default(),
            )
            .unwrap();
            let mut want = ByteWriter::le();
            want.u32(f).u64(0x7ff0_0000_0000_0001);
            assert_eq!(out, want.into_inner(), "{floats:?} [{reader}] write");
        }
    }
}

/// Plan 3.10: a `ReferencedObject` whose data holds another one reads
/// through both ref types, each map keyed by its own tree's list: `A`'s
/// data re-entering `B` puts three lists in `Parsed` (the root's, `A`'s,
/// `B`'s), and `A` re-entering `A` two (a tree is listed once). The writer
/// writes the first back byte for byte; the second ends in an empty class,
/// which only `MirrorReader` writes.
#[test]
fn ref_type_data_reenters_a_ref_type() {
    let mut root_rows = vec![(0, "Base", "Base", 0)];
    root_rows.extend(ref_rows(1, "ref"));
    let root = tree(&root_rows);
    let mut a_rows = vec![(0, "A", "Base", 0), (1, "int", "x", 0)];
    a_rows.extend(ref_rows(1, "inner"));
    let b_rows = [(0, "B", "Base", 0), (1, "int", "y", 0)];
    let refs = [ref_type("A", &a_rows), ref_type("B", &b_rows)];
    let refs = RefTypes::List(&refs);

    let mut w = ByteWriter::le();
    for s in ["A", "n", "a"] {
        string(&mut w, s);
    }
    w.i32(1);
    for s in ["B", "n", "a"] {
        string(&mut w, s);
    }
    w.i32(2);
    let a_then_b = w.into_inner();
    for (reader, p) in read_both(&a_then_b, &root, refs) {
        assert_eq!(p.lists().len(), 3, "{reader}");
        assert_eq!(
            p.get(&[b"ref", b"data", b"x"]),
            Some(&Value::Int(1)),
            "{reader}"
        );
        let y: [&[u8]; 5] = [b"ref", b"data", b"inner", b"data", b"y"];
        assert_eq!(p.get(&y), Some(&Value::Int(2)), "{reader}");
        assert_eq!(
            p.bytes_read,
            i64::try_from(a_then_b.len()).unwrap(),
            "{reader}"
        );
        let out = write(&p, &root, refs, WriteRules::UnityPy, &Limits::default());
        assert_eq!(out.unwrap(), a_then_b, "{reader} write");
    }

    let mut w = ByteWriter::le();
    for s in ["A", "n", "a"] {
        string(&mut w, s);
    }
    w.i32(1);
    for s in ["A", "n", "a"] {
        string(&mut w, s);
    }
    w.i32(3);
    for s in ["", "", ""] {
        string(&mut w, s);
    }
    let a_then_a = w.into_inner();
    for (reader, p) in read_both(&a_then_a, &root, refs) {
        assert_eq!(p.lists().len(), 2, "{reader}");
        let x: [&[u8]; 5] = [b"ref", b"data", b"inner", b"data", b"x"];
        assert_eq!(p.get(&x), Some(&Value::Int(3)), "{reader}");
        let none: [&[u8]; 6] = [b"ref", b"data", b"inner", b"data", b"inner", b"data"];
        assert_eq!(p.get(&none), None, "{reader}: an empty class has no data");
        let out = write(
            &p,
            &root,
            refs,
            WriteRules::MirrorReader,
            &Limits::default(),
        );
        assert_eq!(out.unwrap(), a_then_a, "{reader} mirror write");
        let out = write(&p, &root, refs, WriteRules::UnityPy, &Limits::default());
        assert!(
            matches!(out, Err(WriteError::MissingKey(ref k)) if k.as_bytes() == b"data"),
            "{reader} write (W7): {out:?}"
        );
    }
}

/// The rows of a tree five classes deep.
const DEEP: [Row<'static>; 6] = [
    (0, "Base", "Base", 0),
    (1, "C", "c", 0),
    (2, "C", "c", 0),
    (3, "C", "c", 0),
    (4, "C", "c", 0),
    (5, "int", "x", 0),
];

/// Plan 3.10: a value deeper than `max_value_depth` trips to `other`, from
/// both readers, and the bound stops a write too.
#[test]
fn value_depth_trips_to_other() {
    let deep = tree(&DEEP);
    let mut limits = Limits::default();
    limits.max_value_depth = 3;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let one = 7_i32.to_le_bytes();
    for (reader, r) in both(
        &one,
        &deep,
        RefTypes::List(&[]),
        &options,
        &Meter::unlimited(),
    ) {
        assert_limit(&r, LimitKind::ValueDepth, reader);
    }
    for (reader, p) in read_both(&one, &deep, RefTypes::List(&[])) {
        let out = write(&p, &deep, RefTypes::List(&[]), WriteRules::UnityPy, &limits);
        assert!(
            matches!(out, Err(WriteError::Limit(ref l)) if l.kind == LimitKind::ValueDepth),
            "{reader} write: {out:?}"
        );
    }
}

/// Plan 3.10: more values than `max_values_per_object` trips to `other`.
#[test]
fn values_per_object_trip_to_other() {
    let strings = tree(&[
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "string", "data", 0),
    ]);
    let mut w = ByteWriter::le();
    w.i32(200);
    for _ in 0..200 {
        string(&mut w, "");
    }
    let mut limits = Limits::default();
    limits.max_values_per_object = 100;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    for (reader, r) in both(
        &w.into_inner(),
        &strings,
        RefTypes::List(&[]),
        &options,
        &Meter::unlimited(),
    ) {
        assert_limit(&r, LimitKind::ValuesPerObject, reader);
    }
}

/// Plan 3.10: more zero-size elements than `max_zero_size_elements` trip
/// to `other`; as many as the bound read.
#[test]
fn zero_size_elements_trip_to_other() {
    let empties = tree(&[
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "Empty", "data", 0),
    ]);
    let mut limits = Limits::default();
    limits.max_zero_size_elements = 10;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    for (reader, r) in both(
        &11_i32.to_le_bytes(),
        &empties,
        RefTypes::List(&[]),
        &options,
        &Meter::unlimited(),
    ) {
        assert_limit(&r, LimitKind::ZeroSizeElements, reader);
    }
    // Ten fit.
    for (reader, r) in both(
        &10_i32.to_le_bytes(),
        &empties,
        RefTypes::List(&[]),
        &options,
        &Meter::unlimited(),
    ) {
        assert!(r.is_ok(), "{reader}: {r:?}");
    }
}

/// Plan 3.10: a ref type's tree deeper than `max_tree_depth` trips to
/// `other` (`ReadDetail::Tree`).
#[test]
fn tree_depth_trips_to_other() {
    // A ref type's tree deeper than the bound: compiled when the read
    // reaches its data, under the read's limits.
    let mut root_rows = vec![(0, "Base", "Base", 0)];
    root_rows.extend(ref_rows(1, "ref"));
    let root = tree(&root_rows);
    let refs = [ref_type("A", &DEEP)];
    let mut w = ByteWriter::le();
    for s in ["A", "n", "a"] {
        string(&mut w, s);
    }
    w.i32(7);
    let mut limits = Limits::default();
    limits.max_tree_depth = 3;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    for (reader, r) in both(
        &w.into_inner(),
        &root,
        RefTypes::List(&refs),
        &options,
        &Meter::unlimited(),
    ) {
        assert!(
            matches!(
                &r,
                Err(ReadError {
                    kind: ReadErrorKind::Other,
                    detail: ReadDetail::Tree(TreeError::Limit(l)),
                    ..
                }) if l.kind == LimitKind::TreeDepth
            ),
            "{reader}: {r:?}"
        );
    }
}

/// Plan 3.10 (linear dicts) and R13: a registry child skipped because one
/// is in scope sets nothing, so a later child of its name is that name's
/// first set and lands where `PyDict_SetItem` puts it: after `b`, not at
/// the skipped child's place. The writer skips the same child (W6).
#[test]
fn a_name_repeated_after_a_skipped_registry_lands_last() {
    let t = tree(&[
        (0, "Base", "Base", 0),
        (1, "ManagedReferencesRegistry", "references", 0),
        (2, "int", "version", 0),
        (1, "Inner", "inner", 0),
        (2, "int", "a", 0),
        (2, "ManagedReferencesRegistry", "dup", 0),
        (3, "int", "version", 0),
        (2, "int", "b", 0),
        (2, "int", "dup", 0),
    ]);
    let mut w = ByteWriter::le();
    w.i32(1).i32(2).i32(3).i32(4);
    let data = w.into_inner();
    for (reader, p) in read_both(&data, &t, RefTypes::List(&[])) {
        let Some(Value::Dict(inner)) = p.get(&[b"inner"]) else {
            panic!("{reader}: no inner dict");
        };
        let entries: Vec<(&[u8], &Value)> = p.entries(inner).collect();
        assert_eq!(
            entries,
            [
                (&b"a"[..], &Value::Int(2)),
                (&b"b"[..], &Value::Int(3)),
                (&b"dup"[..], &Value::Int(4)),
            ],
            "{reader}"
        );
        let out = write(
            &p,
            &t,
            RefTypes::List(&[]),
            WriteRules::UnityPy,
            &Limits::default(),
        );
        assert_eq!(out.unwrap(), data, "{reader} write");
    }
}

/// Plan 3.10, list growth: a list of 2^16 + 1 strings (one more than the
/// largest preallocation) ends with its capacity equal to its length, from
/// both readers.
#[test]
fn a_list_past_the_preallocation_has_capacity_equal_to_length() {
    const N: usize = (1 << 16) + 1;
    let t = tree(&[
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "string", "data", 0),
    ]);
    let mut w = ByteWriter::le();
    w.i32(i32::try_from(N).unwrap());
    for _ in 0..N {
        string(&mut w, "");
    }
    for (reader, p) in read_both(&w.into_inner(), &t, RefTypes::List(&[])) {
        let Some(Value::List(l)) = p.get(&[b"v"]) else {
            panic!("{reader}: no list");
        };
        assert_eq!(l.len(), N, "{reader}");
        assert_eq!(l.capacity(), N, "{reader}");
    }
}

const TYPES: &[&str] = &[
    "SInt8",
    "UInt8",
    "char",
    "short",
    "SInt16",
    "unsigned short",
    "UInt16",
    "int",
    "SInt32",
    "unsigned int",
    "UInt32",
    "Type*",
    "long long",
    "SInt64",
    "unsigned long long",
    "UInt64",
    "FileSize",
    "float",
    "double",
    "bool",
    "string",
    "TypelessData",
    "pair",
    "Array",
    "ReferencedObject",
    "ReferencedObjectData",
    "ReferencedManagedType",
    "ManagedReferencesRegistry",
    "PPtr<Object>",
    "Base",
    "vector",
    "",
    "int\0x",
];

const NAMES: &[&str] = &[
    "a", "b", "Array", "size", "data", "first", "second", "type", "class", "ns", "asm", "x", "",
];

fn pick<'a>(rng: &mut Rng, from: &[&'a str]) -> &'a str {
    from.get(rng.below(from.len())).copied().unwrap_or_default()
}

/// A random tree of up to 24 nodes, each level at most one below the last.
fn random_rows(rng: &mut Rng) -> Vec<NodeRow> {
    let n = 1 + rng.below(24);
    let mut rows = Vec::with_capacity(n);
    let mut level = 0_i32;
    for i in 0..n {
        if i > 0 {
            level = 1 + i32::try_from(rng.below(usize::try_from(level).unwrap() + 1)).unwrap();
        }
        let meta = if rng.below(3) == 0 { ALIGN } else { 0 };
        rows.push(NodeRow::new(level, pick(rng, TYPES), pick(rng, NAMES)).with_meta_flag(meta));
    }
    rows
}

/// Bytes biased to small integers, so lengths and counts often fit.
fn random_bytes(rng: &mut Rng, n: usize) -> Vec<u8> {
    (0..n)
        .map(|_| match rng.below(10) {
            0..=5 => u8::try_from(rng.below(4)).unwrap(),
            6 | 7 => 0xff,
            _ => u8::try_from(rng.below(256)).unwrap(),
        })
        .collect()
}

/// Reads `data` every way a caller can and writes back what reads, with
/// both rules; nothing may panic. Returns how many reads succeeded.
fn exercise(rng: &mut Rng, data: &[u8], tree: &Arc<Tree>, refs: RefTypes<'_>) -> usize {
    let mut limits = Limits::default();
    limits.max_zero_size_elements = 256;
    limits.max_values_per_object = 4096;
    let options = ReadOptions {
        floats: if rng.below(2) == 0 {
            FloatMode::UnityPy
        } else {
            FloatMode::Lossless
        },
        limits,
    };
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let endian = if rng.below(4) == 0 {
        Endian::Big
    } else {
        Endian::Little
    };
    let ctx = ReadCtx {
        endian,
        ref_types: refs,
        options: &options,
        meter: &meter,
    };
    let file = Bytes::copy_from_slice(data);
    let start = rng.below(data.len() + 1);
    let size = u32::try_from(rng.below(data.len() + 2)).unwrap();
    let start_i = i128::try_from(start).unwrap() - i128::from(rng.below(4) == 0);
    let reads = [
        walk::boost::read(&file.slice(start..), tree, &ctx),
        walk::python::read(&file, start_i, tree, &ctx),
        walk::read_typetree(&file, start_i, size, tree, &ctx, rng.below(2) == 0).map_err(|_| {
            ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python("check_read"),
            }
        }),
    ];
    let mut ok = 0;
    for r in reads {
        let Ok(p) = r else { continue };
        ok += 1;
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let wctx = WriteCtx {
                endian,
                ref_types: refs,
                rules,
                limits: &limits,
            };
            let mut out = DecodeBuf::new(&meter, 0, 1 << 20, LimitKind::InputMemory).unwrap();
            if walk::write::write(&p, tree, &wctx, &mut out).is_ok()
                && rules == WriteRules::MirrorReader
            {
                let again = Bytes::copy_from_slice(out.as_slice());
                let _ = walk::boost::read(&again, tree, &ctx);
            }
        }
    }
    assert!(meter.used() < 1 << 20);
    ok
}

/// Seeded no-panic runs: 10,000 random trees over random bytes and random
/// ref types, every reader and both write rules (plan 7.3).
#[test]
fn random_trees_and_bytes_never_panic() {
    let mut rng = Rng::new(0x5eed_b0b5);
    let (mut compiled, mut ok) = (0, 0);
    for _ in 0..10_000 {
        let Ok(list) = NodeList::trusted(random_rows(&mut rng)) else {
            continue;
        };
        let Ok(t) = Tree::compile(Arc::new(list), &Limits::default()) else {
            continue;
        };
        compiled += 1;
        let refs: Vec<SerializedType> = (0..rng.below(3))
            .map(|_| {
                let nodes = (rng.below(4) != 0)
                    .then(|| NodeList::trusted(random_rows(&mut rng)).ok().map(Arc::new))
                    .flatten();
                SerializedType::ref_type(
                    114,
                    RawStr::from(pick(&mut rng, &["", "a", "b"])),
                    RawStr::from(pick(&mut rng, &["", "a"])),
                    RawStr::from(pick(&mut rng, &["", "a"])),
                    nodes,
                )
            })
            .collect();
        let refs = match rng.below(6) {
            0 => RefTypes::NoFile,
            1 => RefTypes::Absent,
            _ => RefTypes::List(&refs),
        };
        let n = rng.below(48);
        let data = random_bytes(&mut rng, n);
        ok += exercise(&mut rng, &data, &t, refs);
    }
    assert!(compiled > 9_000, "{compiled} trees compiled");
    assert!(ok > 3_000, "only {ok} reads succeeded");
}
