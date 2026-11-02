//! Plan 3.10, "Bounds", and 3.11: the writer counts what it writes against
//! the bound the readers count against, `Limits::max_values_per_object`,
//! and that bound refuses no write whose read it allowed, unless the tree
//! repeats a name, or a node whose type name holds a NUL has a child that
//! writes no byte.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_bounds
//!
//! Every case `tools/fixtures/walk.py` records (`tests/fixtures/walk/cases.json`)
//! is read by both readers, then read again with the bound at the least
//! the read passes under, found by bisection: the read then counts exactly
//! that much. Its value, written under that bound with either rule, has the
//! outcome it has under the default limits: a write through the tree it
//! was read with counts no more than its read. UnityPyBoost's read counts
//! each child it skips and each value under a name a sibling repeats; the
//! pure-Python reader every value and every element of a primitive run;
//! the writer each child it skips, each value under a repeated name and
//! each child that writes no byte of a class it writes from a value that
//! is not a dict (`walk::Work`'s docs). A tree that repeats a name hands
//! one value to every child of the name, each of which writes it whole, so
//! a write can count more than its read; the last test pins one that does.
//! A node whose type name holds a NUL is read by UnityPyBoost as the name
//! before it (a primitive, a string or bytes) and written as a class from
//! that value, its children the value's attributes: one that writes no byte
//! counts where the read counted nothing (`walk::linear_tests`).

use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::LimitKind;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::error::LimitError;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ReadCtx, ReadError, ReadOptions, RefTypes, WriteCtx, WriteError, WriteRules,
};
use unityrs_core::{Bytes, Parsed, RawStr};

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

/// `[[level, type, name, meta], ...]` as a node list.
fn nodes(rows: &Json) -> Arc<NodeList> {
    let rows = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
                .with_meta_flag(i32::try_from(r[3].as_i64().unwrap()).unwrap())
        })
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

/// A case's ref types as a list: `null` (no file) and `"absent"` read as
/// an empty list here, which changes no outcome this test compares (the
/// read and the write are each run on one list).
fn ref_types(spec: Option<&Json>) -> Vec<SerializedType> {
    let Some(Json::Array(a)) = spec else {
        return Vec::new();
    };
    a.iter()
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
        .collect()
}

/// Whether a tree repeats a name among a node's children.
fn repeats_a_name(tree: &Tree) -> bool {
    (0..tree.list().len()).any(|i| {
        let id = NodeId(u32::try_from(i).unwrap());
        tree.info(id).is_some_and(|x| x.key != id)
    })
}

fn limits(max_values: u64) -> Limits {
    let mut l = Limits::default();
    l.max_values_per_object = max_values;
    l
}

fn read(
    reader: &str,
    data: &Bytes,
    start: usize,
    tree: &Arc<Tree>,
    refs: &[SerializedType],
    endian: Endian,
    max_values: u64,
) -> Result<Parsed, ReadError> {
    let options = ReadOptions {
        limits: limits(max_values),
        ..ReadOptions::default()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian,
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    match reader {
        "boost" => walk::boost::read(&data.slice(start..), tree, &ctx),
        _ => walk::python::read(data, i128::try_from(start).unwrap(), tree, &ctx),
    }
}

fn write(
    p: &Parsed,
    tree: &Tree,
    refs: &[SerializedType],
    endian: Endian,
    rules: WriteRules,
    max_values: u64,
) -> Result<Vec<u8>, WriteError> {
    let limits = limits(max_values);
    let ctx = WriteCtx {
        endian,
        ref_types: RefTypes::List(refs),
        rules,
        limits: &limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    walk::write::write(p, tree, &ctx, &mut out).map(|()| out.as_slice().to_vec())
}

/// The least bound `read_at` passes under, by bisection (a read counts the
/// same values under any bound it passes).
fn least_bound(read_at: impl Fn(u64) -> bool) -> u64 {
    let (mut lo, mut hi) = (0, Limits::default().max_values_per_object);
    assert!(read_at(hi));
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if read_at(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[test]
fn a_write_counts_no_more_values_than_its_read() {
    let cases = load("cases.json");
    let default = Limits::default().max_values_per_object;
    let (mut n, mut bad) = (0, Vec::new());
    for c in cases.as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let tree = Tree::compile(nodes(&c["nodes"]), &Limits::default()).unwrap();
        let refs = ref_types(c.get("ref_types"));
        let ref_repeats = refs.iter().any(|r| match r.tree(&Limits::default()) {
            Some(Ok(t)) => repeats_a_name(&t),
            _ => false,
        });
        if repeats_a_name(&tree) || ref_repeats {
            continue;
        }
        let endian = match c.get("endian").and_then(Json::as_str) {
            Some(">") => Endian::Big,
            _ => Endian::Little,
        };
        let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        let start = usize::try_from(c.get("start").and_then(Json::as_i64).unwrap_or(0)).unwrap();
        for reader in ["boost", "python"] {
            let read_at = |max| read(reader, &data, start, &tree, &refs, endian, max);
            let Ok(p) = read_at(default) else {
                continue;
            };
            n += 1;
            let least = least_bound(|max| read_at(max).is_ok());
            let p_least = read_at(least).unwrap();
            for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
                let want = write(&p, &tree, &refs, endian, rules, default);
                let have = write(&p_least, &tree, &refs, endian, rules, least);
                if have != want {
                    bad.push(format!(
                        "{name} [{reader} read of {least} values, {rules:?}]: {have:?}, not {want:?}"
                    ));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} writes refused where the read passed:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(n > 200, "only {n} reads");
}

/// Whether a tree can skip a child (a `ManagedReferencesRegistry` once one
/// is in scope, a `ReferencedObject`'s data) or repeats a name: what
/// UnityPyBoost's read and the writer count against the bound.
fn counts_against_the_bound(tree: &Tree) -> bool {
    repeats_a_name(tree)
        || tree.list().rows().iter().any(|r| {
            matches!(
                r.ty.as_bytes(),
                b"ManagedReferencesRegistry" | b"ReferencedObjectData"
            )
        })
}

/// Review round 9: every value UnityPyBoost's read keeps sits in a slot the
/// meter charges, and a primitive run is one value of the object's bytes,
/// so the read counts none of them against `max_values_per_object`, nor
/// does a write of its value through the same tree count a value it
/// writes. Every case whose tree (and ref types') skips nothing and repeats
/// no name reads, and writes under either rule, under a bound of 0 exactly
/// as under the default (a struct array, a run and a string list among
/// them), where the bound counted each value and each run's element (a
/// `Mesh` of a 2^26 + 1 byte `vector<UInt8>` was refused at the default);
/// so do the `w1_nul_type_name_` cases, a run, strings and bytes read
/// through a type name holding a NUL, and one whose node has a child that
/// writes its bytes, which the writer writes as classes (review round 10:
/// it counted each such class, so the re-save of a run past the default was
/// refused where UnityPy writes it, `tests/walk_runs.rs`).
/// The pure-Python reader still counts every value: each of those cases
/// that builds one is refused under 0.
#[test]
fn a_boost_read_and_its_write_count_no_value_they_keep() {
    let cases = load("cases.json");
    let default = Limits::default().max_values_per_object;
    let (mut n, mut bad) = (0, Vec::new());
    for c in cases.as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let tree = Tree::compile(nodes(&c["nodes"]), &Limits::default()).unwrap();
        let refs = ref_types(c.get("ref_types"));
        let ref_counts = refs.iter().any(|r| match r.tree(&Limits::default()) {
            Some(Ok(t)) => counts_against_the_bound(&t),
            _ => false,
        });
        if counts_against_the_bound(&tree) || ref_counts {
            continue;
        }
        let endian = match c.get("endian").and_then(Json::as_str) {
            Some(">") => Endian::Big,
            _ => Endian::Little,
        };
        let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        let start = usize::try_from(c.get("start").and_then(Json::as_i64).unwrap_or(0)).unwrap();
        let read_at = |reader, max| read(reader, &data, start, &tree, &refs, endian, max);
        let Ok(want) = read_at("boost", default) else {
            continue;
        };
        n += 1;
        match read_at("boost", 0) {
            Ok(p) if p.value() == want.value() && p.bytes_read == want.bytes_read => {}
            other => bad.push(format!("{name} [boost read]: {other:?}")),
        }
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let w = write(&want, &tree, &refs, endian, rules, default);
            let have = write(&want, &tree, &refs, endian, rules, 0);
            if have != w {
                bad.push(format!("{name} [{rules:?} write]: {have:?}, not {w:?}"));
            }
        }
        if read_at("python", default).is_ok() {
            assert!(
                matches!(read_at("python", 0), Err(ReadError { detail: walk::ReadDetail::Limit(l), .. })
                    if l.kind == LimitKind::ValuesPerObject),
                "{name}: the pure-Python reader under 0"
            );
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {n} cases differ under a bound of 0:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(n > 60, "only {n} reads");
}

/// The exception: `Base { vector<int> a; vector<int> a }` over an empty
/// vector and one of five ints. UnityPyBoost's read counts the two vectors,
/// both under the repeated name, each run one value; the pure-Python
/// reader counts eight (the class, the two vectors and the second's five
/// ints). Both keep the second vector, which the write writes through both
/// children, each a vector and its five ints under the repeated name:
/// twelve, so under the least bound either read passes the write is
/// refused, and it passes under its own twelve.
#[test]
fn a_repeated_name_can_write_more_values_than_its_read() {
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    for _ in 0..2 {
        rows.push(NodeRow::new(1, "vector", "a"));
        rows.push(NodeRow::new(2, "Array", "Array"));
        rows.push(NodeRow::new(3, "int", "size"));
        rows.push(NodeRow::new(3, "int", "data"));
    }
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    assert!(repeats_a_name(&tree));
    let mut bytes = 0_i32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&5_i32.to_le_bytes());
    for i in 0..5_i32 {
        bytes.extend_from_slice(&i.to_le_bytes());
    }
    let data = Bytes::from(bytes);
    for (reader, counted) in [("boost", 2), ("python", 8)] {
        let read_at = |max| read(reader, &data, 0, &tree, &[], Endian::Little, max);
        let least = least_bound(|max| read_at(max).is_ok());
        assert_eq!(least, counted, "{reader}");
        let p = read_at(least).unwrap();
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            for max in [least, 11] {
                assert_eq!(
                    write(&p, &tree, &[], Endian::Little, rules, max),
                    Err(WriteError::Limit(LimitError {
                        kind: LimitKind::ValuesPerObject,
                        value: max + 1,
                        max,
                    })),
                    "{reader} read, {rules:?} write under {max}"
                );
            }
            assert!(
                write(&p, &tree, &[], Endian::Little, rules, 12).is_ok(),
                "{reader} read, {rules:?} write"
            );
        }
    }
}
