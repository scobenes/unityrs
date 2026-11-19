//! The direct read (`walk::emit`) against UnityPyBoost's reader
//! (`walk::boost`) on every walk fixture: where it reads, the same values,
//! position and work; where it gives up, a tree it does not take or a read
//! the full reader fails.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_emit

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::limits::Meter;
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, Tree};
use unityrs_core::walk::emit::{self, Run, Scalar, Sink};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed};

#[allow(dead_code, reason = "the ref types' helpers are other tests'")]
mod walk_common;

use walk_common::{compile, enc, endian, float, hex, load, unhex};

type Obj = Rc<RefCell<Json>>;

/// Builds each value as the fixture encodes the Python object.
struct JsonSink {
    tree: Arc<Tree>,
    rooms: Vec<usize>,
}

impl JsonSink {
    fn new(tree: &Arc<Tree>) -> Self {
        Self {
            tree: tree.clone(),
            rooms: Vec::new(),
        }
    }
}

fn scalar(v: Scalar) -> Json {
    match v {
        Scalar::Bool(v) => json!(v),
        Scalar::Int(v) => json!(v),
        Scalar::UInt(v) => json!(v),
        Scalar::Float(v) => float(v.to_bits()),
    }
}

#[allow(clippy::unnecessary_wraps, reason = "a sink's every result")]
fn obj(v: Json) -> Option<Obj> {
    Some(Rc::new(RefCell::new(v)))
}

fn take(o: Obj) -> Json {
    Rc::try_unwrap(o).unwrap().into_inner()
}

impl Sink for JsonSink {
    type Obj = Obj;
    fn scalar(&mut self, v: Scalar) -> Option<Obj> {
        obj(scalar(v))
    }
    fn str(&mut self, v: &[u8]) -> Option<Obj> {
        obj(json!({ "s": hex(v) }))
    }
    fn bytes(&mut self, v: &[u8]) -> Option<Obj> {
        obj(json!({ "y": hex(v) }))
    }
    fn tuple(&mut self, first: Obj, second: Obj) -> Option<Obj> {
        obj(json!({ "t": [take(first), take(second)] }))
    }
    fn dict(&mut self, _node: NodeId, _children: usize) -> Option<Obj> {
        obj(json!({ "d": [] }))
    }
    fn set(&mut self, dict: &Obj, key: NodeId, value: Obj) -> Option<()> {
        let name = String::from_utf8(self.tree.list().name(key).to_vec()).unwrap();
        dict.borrow_mut()["d"]
            .as_array_mut()?
            .push(json!([name, take(value)]));
        Some(())
    }
    fn list(&mut self, items: std::vec::Drain<'_, Obj>) -> Option<Obj> {
        obj(Json::Array(items.map(take).collect()))
    }
    fn run(&mut self, run: Run<'_>) -> Option<Obj> {
        let mut items = Vec::new();
        run.each(|v| {
            items.push(scalar(v));
            Some(())
        })?;
        assert_eq!(items.len(), run.len());
        obj(Json::Array(items))
    }
    fn room(&mut self, items: usize) -> bool {
        self.rooms.push(items);
        true
    }
}

/// UnityPyBoost's read of `object` and the work it counted.
fn boost(object: &Bytes, c: &Json, tree: &Arc<Tree>) -> (Option<Parsed>, u64) {
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: endian(c),
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    let got = walk::boost::read(object, tree, &ctx).ok();
    (got, meter.decoded())
}

/// Replays one case; a difference is its message.
fn replay(c: &Json, data: &Bytes, read: &mut usize) -> Option<String> {
    let name = c["name"].as_str().unwrap();
    let tree = compile(&c["nodes"]);
    let (want, work) = boost(data, c, &tree);
    let mut sink = JsonSink::new(&tree);
    let options = ReadOptions::default();
    let got = emit::emit(
        data,
        &tree,
        endian(c) == Endian::Big,
        options.floats,
        options.limits.max_value_depth,
        &mut sink,
    );
    match (got, want) {
        (Some(got), Some(want)) => {
            *read += 1;
            let have = json!([take(got.value), got.bytes_read, got.work]);
            let want = json!([enc(&want, want.value()), want.bytes_read, work]);
            (have != want).then(|| format!("{name}\n  want {want}\n  have {have}"))
        }
        (Some(_), None) => Some(format!("{name}: read where UnityPyBoost's reader fails")),
        (None, Some(_)) if emit::supported(&tree) => Some(format!(
            "{name}: gave up a read UnityPyBoost's reader completes"
        )),
        (None, _) => None,
    }
}

fn cases(file: &str) -> Vec<(Json, Bytes)> {
    load(file)
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| {
            let data = unhex(c.get("hex")?.as_str()?);
            let start = c.get("start").and_then(Json::as_u64).unwrap_or(0);
            let data = data.get(usize::try_from(start).ok()?..)?.to_vec();
            Some((c.clone(), Bytes::from(data)))
        })
        .collect()
}

#[test]
fn emit_matches_boost_on_every_fixture() {
    let mut bad = Vec::new();
    let (mut n, mut read) = (0, 0);
    for file in ["cases.json", "value_cases.json"] {
        for (c, data) in cases(file) {
            n += 1;
            bad.extend(replay(&c, &data, &mut read));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(read > 100, "only {read} of {n} cases were read directly");
}

/// Every element kind of a run, both byte orders, against UnityPyBoost's.
#[test]
fn runs_of_every_kind() {
    let kinds = [
        ("UInt8", 1),
        ("SInt8", 1),
        ("bool", 1),
        ("UInt16", 2),
        ("SInt16", 2),
        ("unsigned int", 4),
        ("int", 4),
        ("UInt64", 8),
        ("SInt64", 8),
        ("float", 4),
        ("double", 8),
    ];
    let mut read = 0;
    for (ty, width) in kinds {
        for big in [false, true] {
            for flag in [0, 0x4000] {
                let count = 5_usize;
                let mut data = Vec::new();
                let len = u32::try_from(count).unwrap();
                data.extend(if big {
                    len.to_be_bytes()
                } else {
                    len.to_le_bytes()
                });
                // NaN payloads, signalling ones among them, and extremes.
                for i in 0..count * width {
                    data.push([0xff, 0x80, 0x7f, 0x01, 0xa5, 0x00, 0xc0][i % 7]);
                }
                data.extend([7, 0, 0, 0, 9, 0, 0, 0]);
                let c = json!({
                    "name": format!("{ty} {big} {flag}"),
                    "endian": if big { ">" } else { "<" },
                    "nodes": [
                        [0, "Base", "Base", 0],
                        [1, "vector", "v", 0],
                        [2, "Array", "Array", 0],
                        [3, "int", "size", 0],
                        [3, ty, "data", flag],
                        [1, "int", "after", 0],
                    ],
                });
                let bad = replay(&c, &Bytes::from(data), &mut read);
                assert!(bad.is_none(), "{}", bad.unwrap());
            }
        }
    }
    assert_eq!(read, 44);
}

/// A list longer than its bytes can hold, or a sink's refused room, gives
/// up before any element is read.
#[test]
fn gives_up_before_reading_what_cannot_fit() {
    let c = json!({
        "nodes": [
            [0, "Base", "Base", 0],
            [1, "vector", "v", 0],
            [2, "Array", "Array", 0],
            [3, "int", "size", 0],
            [3, "Vector3f", "data", 0],
            [4, "float", "x", 0],
            [4, "float", "y", 0],
            [4, "float", "z", 0],
        ],
    });
    let tree = compile(&c["nodes"]);
    let mut data = 1000_u32.to_le_bytes().to_vec();
    data.extend([0; 24]);
    let mut sink = JsonSink::new(&tree);
    let got = emit::emit(
        &data,
        &tree,
        false,
        ReadOptions::default().floats,
        512,
        &mut sink,
    );
    assert!(got.is_none());
    assert!(sink.rooms.is_empty(), "{:?}", sink.rooms);
}

/// Trees with a registry, a `ReferencedObject` or a repeated name are not
/// taken.
#[test]
fn unsupported_trees() {
    for rows in [
        json!([
            [0, "Base", "Base", 0],
            [1, "int", "a", 0],
            [1, "int", "a", 0]
        ]),
        json!([
            [0, "Base", "Base", 0],
            [1, "ManagedReferencesRegistry", "r", 0],
            [2, "int", "version", 0]
        ]),
        json!([
            [0, "Base", "Base", 0],
            [1, "ReferencedObject", "o", 0],
            [2, "int", "rid", 0]
        ]),
    ] {
        assert!(!emit::supported(&compile(&rows)), "{rows}");
    }
    let plain = json!([
        [0, "Base", "Base", 0],
        [1, "int", "a", 0],
        [1, "int", "b", 0]
    ]);
    assert!(emit::supported(&compile(&plain)));
}
