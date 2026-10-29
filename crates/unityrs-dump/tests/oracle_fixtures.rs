//! Replays `tools/fixtures/dump.py`'s oracle fixtures without Python (plan
//! section 6, G4): `canonical.py`'s encodings of synthetic values and names,
//! and Python's decimal form of packed integers.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::typetree::{NodeId, NodeList, NodeRow};
use unityrs_core::{Built, Bytes, Parsed, PrimArray, PrimRun, RawStr, Value, ValueBuilder};
use unityrs_dump::encode::encode;
use unityrs_dump::json::{limbs_to_decimal, write_name};

fn fixture(name: &str) -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dump")
        .join(name);
    let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&text).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn s(v: &Json) -> &str {
    v.as_str().unwrap()
}

// ------------------------------------------------------------------ values

/// Every dict key name in a description, in the order `build` meets them.
fn key_names(desc: &Json, out: &mut Vec<Vec<u8>>) {
    let d = desc.as_array().unwrap();
    match s(&d[0]) {
        "list" | "tuple_array" => {
            for item in d[2].as_array().unwrap() {
                key_names(item, out);
            }
        }
        "pair" => {
            key_names(&d[1], out);
            key_names(&d[2], out);
        }
        "dict" => {
            for entry in d[1].as_array().unwrap() {
                let e = entry.as_array().unwrap();
                out.push(unhex(s(&e[0])));
                key_names(&e[1], out);
            }
        }
        _ => {}
    }
}

fn int(text: &str) -> Value {
    let v: i128 = text.parse().unwrap();
    match i64::try_from(v) {
        Ok(i) => Value::Int(i),
        Err(_) => Value::UInt(u64::try_from(v).unwrap()),
    }
}

fn run<T>(b: &ValueBuilder, items: &[Json], f: impl Fn(&Json) -> T) -> MeteredVec<T> {
    let mut v = b.vec();
    for x in items {
        v.push(f(x)).unwrap();
    }
    v
}

/// The `Value` the reader makes for a description, with its charge.
fn build(b: &ValueBuilder, desc: &Json, next_key: &mut u32) -> Built {
    let d = desc.as_array().unwrap();
    let scalar = |v: Value| b.scalar(v).unwrap();
    match s(&d[0]) {
        "bool" => scalar(Value::Bool(d[1].as_bool().unwrap())),
        "int" => scalar(int(s(&d[1]))),
        "f32" => scalar(Value::F32(u32::from_str_radix(s(&d[1]), 16).unwrap())),
        "f64" => scalar(Value::F64(u64::from_str_radix(s(&d[1]), 16).unwrap())),
        "str" => scalar(Value::Str(RawStr::from(unhex(s(&d[1]))))),
        "bytes" => scalar(Value::Bytes(Bytes::from(unhex(s(&d[1]))))),
        "prims" => {
            let elem = s(&d[1]);
            if elem == "u8" || elem == "char" {
                return scalar(Value::Prims(PrimArray::U8(Bytes::from(unhex(s(&d[2]))))));
            }
            let items = d[2].as_array().unwrap();
            let p = |x: &Json| s(x).parse::<i128>().unwrap();
            let r = match elem {
                "bool" => PrimRun::Bool(run(b, items, |x| x.as_bool().unwrap())),
                "i8" => PrimRun::I8(run(b, items, |x| i8::try_from(p(x)).unwrap())),
                "u16" => PrimRun::U16(run(b, items, |x| u16::try_from(p(x)).unwrap())),
                "i16" => PrimRun::I16(run(b, items, |x| i16::try_from(p(x)).unwrap())),
                "u32" => PrimRun::U32(run(b, items, |x| u32::try_from(p(x)).unwrap())),
                "i32" => PrimRun::I32(run(b, items, |x| i32::try_from(p(x)).unwrap())),
                "u64" => PrimRun::U64(run(b, items, |x| u64::try_from(p(x)).unwrap())),
                "i64" => PrimRun::I64(run(b, items, |x| i64::try_from(p(x)).unwrap())),
                "f32" => PrimRun::F32(run(b, items, |x| u32::from_str_radix(s(x), 16).unwrap())),
                "f64" => PrimRun::F64(run(b, items, |x| u64::from_str_radix(s(x), 16).unwrap())),
                other => panic!("unknown element {other}"),
            };
            b.prims(r).unwrap()
        }
        kind @ ("list" | "tuple_array") => {
            let mut v = b.vec();
            for item in d[2].as_array().unwrap() {
                v.push_built(build(b, item, next_key)).unwrap();
            }
            if kind == "list" {
                b.list(v)
            } else {
                b.tuple(v).unwrap()
            }
        }
        "pair" => {
            let mut v = b.vec();
            v.push_built(build(b, &d[1], next_key)).unwrap();
            v.push_built(build(b, &d[2], next_key)).unwrap();
            b.tuple(v).unwrap()
        }
        "dict" => {
            let mut v = b.vec();
            for entry in d[1].as_array().unwrap() {
                let e = entry.as_array().unwrap();
                let key = NodeId(*next_key);
                *next_key += 1;
                let item = build(b, &e[1], next_key);
                v.push_entry(key, item).unwrap();
            }
            b.map(0, v)
        }
        other => panic!("unknown description {other}"),
    }
}

/// A `Parsed` for a description: its keys name rows 1.. of a flat list.
fn parsed(desc: &Json) -> Parsed {
    let mut names = Vec::new();
    key_names(desc, &mut names);
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    rows.extend(
        names
            .into_iter()
            .map(|n| NodeRow::new(1, "int", RawStr::from(n))),
    );
    let list = Arc::new(NodeList::trusted(rows).unwrap());
    let b = ValueBuilder::new(&Meter::unlimited(), list).unwrap();
    let mut next_key = 1;
    let v = build(&b, desc, &mut next_key);
    b.finish(v, 0)
}

#[test]
fn values_encode_as_canonical_py_encodes_them() {
    let f = fixture("encode.json");
    let cases = f["values"].as_array().unwrap();
    assert!(cases.len() >= 60, "{} value cases", cases.len());
    let mut failed = Vec::new();
    for c in cases {
        let got = encode(&parsed(&c["value"])).unwrap();
        if got != s(&c["encoded"]) {
            failed.push(format!(
                "{}:\n  got  {got}\n  want {}",
                s(&c["name"]),
                s(&c["encoded"])
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn names_encode_as_canonical_py_encodes_them() {
    let f = fixture("encode.json");
    for c in f["names"].as_array().unwrap() {
        let mut got = String::new();
        write_name(&mut got, &unhex(s(&c["hex"])));
        assert_eq!(got, s(&c["encoded"]), "name {}", s(&c["hex"]));
    }
}

#[test]
fn packed_integers_print_exactly_as_python_prints_them() {
    let f = fixture("encode.json");
    let ints = f["ints"].as_array().unwrap();
    // UnityPy's packing of a 4,300-digit group, past 4,300 digits itself.
    assert!(ints.iter().any(|c| s(&c["decimal"]).len() > 4300));
    for c in ints {
        let limbs: Vec<u64> = c["limbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| u64::from_str_radix(s(l), 16).unwrap())
            .collect();
        let packed = unityrs_core::version::Packed::from_limbs(&limbs);
        let mut got = String::new();
        unityrs_dump::json::write_packed(&mut got, &packed);
        assert_eq!(got, s(&c["decimal"]));
        assert_eq!(limbs_to_decimal(&limbs), s(&c["decimal"]));
    }
}
