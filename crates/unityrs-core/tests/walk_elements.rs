//! Arrays at and past the zero-size element bound (plan section 3.10,
//! "Bounds"), replayed against UnityPy 1.25.2 under the limits
//! `EnvOptions::unitypy()` reads with:
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_elements
//!
//! `tests/fixtures/walk/elements.json` is recorded by `tools/fixtures/walk.py`
//! from the cases of `tools/fixtures/walk_elements.py`, whose module docs
//! give the recipe and the encoding. Each case holds 2^20 or 2^20 + 1
//! elements, `Limits::max_zero_size_elements` at its default. An element
//! that reads bytes is read at any count, as UnityPy reads it (review round
//! 4: a malformed `pair` or `Array`, which only the pure-Python reader
//! reads, and a class whose only bytes are its registry's were refused,
//! their compiled minimum being 0). An element that reads nothing (an empty
//! class, the registry class with a registry in scope, a pure-Python
//! element whose negative `TypelessData` length takes the reader back) is
//! refused one past the bound in the readers the case names (`PARITY.md`
//! 19.3), and with the bound lifted is read as UnityPy reads it. Each value
//! read is written back, and compared with UnityPy's re-save.

use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ReadCtx, ReadDetail, ReadError, ReadErrorKind, ReadOptions, RefTypes, WriteCtx,
    WriteRules,
};
use unityrs_core::{Bytes, LimitKind, Map, Parsed, PrimArray, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/walk/elements.json"
);

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn compile(rows: &Json) -> Arc<Tree> {
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
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// The case's bytes: each run's hex, repeated.
fn data(runs: &Json) -> Bytes {
    let mut out = Vec::new();
    for run in runs.as_array().unwrap() {
        let chunk = unhex(run[1].as_str().unwrap());
        for _ in 0..run[0].as_u64().unwrap() {
            out.extend_from_slice(&chunk);
        }
    }
    Bytes::from(out)
}

/// Equal items in a row as one `[count, item]`.
fn runs(items: impl Iterator<Item = Json>) -> Json {
    let mut out: Vec<(u64, Json)> = Vec::new();
    for e in items {
        match out.last_mut() {
            Some((n, last)) if *last == e => *n += 1,
            _ => out.push((1, e)),
        }
    }
    let runs: Vec<Json> = out.into_iter().map(|(n, e)| json!([n, e])).collect();
    json!({ "runs": runs })
}

/// A value as `walk_elements.py` encodes the object UnityPy made (only the
/// kinds these cases build).
fn enc(p: &Parsed, value: &Value) -> Json {
    match value {
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::Bytes(b) => json!({ "y": hex(b) }),
        Value::Prims(PrimArray::I32(v)) => runs(v.iter().map(|x| json!(x))),
        Value::List(v) => runs(v.iter().map(|x| enc(p, x))),
        Value::Tuple(v) => json!({ "t": v.iter().map(|x| enc(p, x)).collect::<Vec<_>>() }),
        Value::Dict(m) => json!({ "d": dict(p, m) }),
        other => panic!("no case builds {other:?}"),
    }
}

fn dict(p: &Parsed, m: &Map) -> Vec<Json> {
    p.entries(m)
        .map(|(k, v)| json!([String::from_utf8(k.to_vec()).unwrap(), enc(p, v)]))
        .collect()
}

/// Bytes as runs of equal 4-byte words.
fn words(b: &[u8]) -> Json {
    let w = runs(b.chunks(4).map(|c| json!(hex(c))));
    json!({ "words": w["runs"] })
}

fn kind(k: ReadErrorKind) -> &'static str {
    match k {
        ReadErrorKind::OutOfBounds => "out_of_bounds",
        ReadErrorKind::NegativeLength => "negative_length",
        ReadErrorKind::RefType => "ref_type",
        ReadErrorKind::Other => "other",
    }
}

fn outcome(r: &Result<Parsed, ReadError>) -> Json {
    match r {
        Ok(p) => json!({ "ok": enc(p, p.value()), "pos": p.bytes_read }),
        Err(e) => json!({ "kind": kind(e.kind) }),
    }
}

fn expected(want: &Json) -> Json {
    if want.get("err").is_some() {
        json!({ "kind": want["kind"] })
    } else {
        want.clone()
    }
}

fn write_out(p: &Parsed, tree: &Tree, limits: &Limits) -> Json {
    let ctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        rules: WriteRules::UnityPy,
        limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    match walk::write::write(p, tree, &ctx, &mut out) {
        Ok(()) => words(out.as_slice()),
        Err(_) => json!({ "err": true }),
    }
}

fn expected_write(want: &Json) -> Json {
    if want.get("err").is_some() {
        json!({ "err": true })
    } else {
        want.clone()
    }
}

/// One reader's read of the whole of `data` (UnityPyBoost's as an object
/// of that size, the pure-Python one's from 0).
fn read(reader: &str, data: &Bytes, tree: &Arc<Tree>, limits: Limits) -> Result<Parsed, ReadError> {
    let options = ReadOptions {
        limits,
        ..EnvOptions::unitypy().context.read_options()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter: &meter,
    };
    if reader == "boost" {
        walk::boost::read(data, tree, &ctx)
    } else {
        walk::python::read(data, 0, tree, &ctx)
    }
}

/// Replays the cases whose `refused` list is empty (`refused == false`) or
/// not; the mismatches.
fn replay(refused: bool) -> (usize, Vec<String>) {
    let text = std::fs::read_to_string(FIXTURE).unwrap();
    let cases: Json = serde_json::from_str(&text).unwrap();
    let limits = EnvOptions::unitypy().context.limits;
    assert_eq!(limits.max_zero_size_elements, 1 << 20, "the cases' bound");
    let mut n = 0;
    let mut bad = Vec::new();
    for c in cases.as_array().unwrap() {
        let refusers: Vec<&str> = c["refused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap())
            .collect();
        if refusers.is_empty() == refused {
            continue;
        }
        n += 1;
        let name = c["name"].as_str().unwrap();
        let tree = compile(&c["nodes"]);
        let data = data(&c["runs"]);
        for reader in ["boost", "python"] {
            let mut lifted = limits;
            if refusers.contains(&reader) {
                match read(reader, &data, &tree, limits) {
                    Err(ReadError {
                        detail: ReadDetail::Limit(l),
                        ..
                    }) if l.kind == LimitKind::ZeroSizeElements
                        && l.value == l.max + 1
                        && l.max == limits.max_zero_size_elements => {}
                    got => bad.push(format!(
                        "{name} [{reader}]: not refused one past the bound: {}",
                        outcome(&got)
                    )),
                }
                lifted.max_zero_size_elements = u64::MAX;
            }
            let got = read(reader, &data, &tree, lifted);
            let want = expected(&c[reader]);
            let have = outcome(&got);
            if have != want {
                bad.push(format!("{name} [{reader}]\n  want {want}\n  have {have}"));
                continue;
            }
            if let Ok(p) = got {
                let want = expected_write(&c[format!("{reader}_write")]);
                let have = write_out(&p, &tree, &limits);
                if have != want {
                    bad.push(format!(
                        "{name} [{reader} write]\n  want {want}\n  have {have}"
                    ));
                }
            }
        }
    }
    (n, bad)
}

/// Elements that read bytes are read at any count, by both readers, as
/// UnityPy reads them, and written back as UnityPy writes them.
#[test]
fn elements_that_read_bytes_are_read_past_the_zero_size_bound() {
    let (n, bad) = replay(false);
    assert_eq!(n, 4, "the cases that read every element");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Elements that read nothing are refused one past the bound by the
/// readers each case names, and read as UnityPy reads them with it lifted.
#[test]
fn elements_that_read_nothing_are_refused_past_the_bound() {
    let (n, bad) = replay(true);
    assert_eq!(n, 3, "the cases refused");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
