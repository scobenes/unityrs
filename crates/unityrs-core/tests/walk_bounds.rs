//! Plan 3.10, "Bounds", and 3.11: the writer counts the values it writes
//! against the bound the readers count theirs against,
//! `Limits::max_values_per_object`, and that bound refuses no write whose
//! read it allowed, unless the tree repeats a name.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_bounds
//!
//! Every case `tools/fixtures/walk.py` records (`tests/fixtures/walk/cases.json`)
//! is read by both readers, then read again with the bound at the least
//! the read passes under, found by bisection: the read then counts exactly
//! that many values. Its value, written under that bound with either rule,
//! has the outcome it has under the default limits: a write through the
//! tree it was read with counts no more values than its read, one a value
//! or a skipped child. A tree that repeats a name hands one value to every
//! child of the name, each of which writes it whole, so a write can count
//! more than its read; the last test pins one that does.

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

/// The exception: `Base { vector<int> a; vector<int> a }` over an empty
/// vector and one of five ints reads eight values (the class, the two
/// vectors and the second's five ints), and keeps the second vector, which
/// the write writes through both children: thirteen values, so under the
/// least bound its read passes the write is refused, and under twice that
/// it is not.
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
    for reader in ["boost", "python"] {
        let read_at = |max| read(reader, &data, 0, &tree, &[], Endian::Little, max);
        let least = least_bound(|max| read_at(max).is_ok());
        assert_eq!(least, 8, "{reader}: the class, two vectors and five ints");
        let p = read_at(least).unwrap();
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            assert_eq!(
                write(&p, &tree, &[], Endian::Little, rules, least),
                Err(WriteError::Limit(LimitError {
                    kind: LimitKind::ValuesPerObject,
                    value: least + 1,
                    max: least,
                })),
                "{reader} read, {rules:?} write"
            );
            assert!(
                write(&p, &tree, &[], Endian::Little, rules, 2 * least).is_ok(),
                "{reader} read, {rules:?} write"
            );
        }
    }
}
