//! What the tests that replay `tests/fixtures/walk/` share: the fixture's
//! encodings (`tools/fixtures/walk.py`'s module docs), a case's tree and ref
//! types, and a write's outcome.

use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, RefTypes, WriteCtx, WriteError, WriteRules};
use unityrs_core::{LimitKind, Map, Parsed, PrimArray, RawStr, Value};

pub const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/walk/");

pub fn load(name: &str) -> Json {
    let text = std::fs::read_to_string(format!("{DIR}{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// `[[level, type, name, meta], ...]` as a node list.
pub fn nodes(rows: &Json) -> Arc<NodeList> {
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

pub fn compile(rows: &Json) -> Arc<Tree> {
    Tree::compile(nodes(rows), &Limits::default()).unwrap()
}

/// A case's ref types: `null` is no file, `"absent"` a file without
/// `ref_types`, a list the file's ref types.
pub enum Refs {
    NoFile,
    Absent,
    List(Vec<SerializedType>),
}

impl Refs {
    pub fn of(spec: Option<&Json>) -> Self {
        match spec {
            None => Self::List(Vec::new()),
            Some(Json::Null) => Self::NoFile,
            Some(Json::String(s)) if s == "absent" => Self::Absent,
            Some(Json::Array(a)) => Self::List(
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
                    .collect(),
            ),
            Some(other) => panic!("ref types {other}"),
        }
    }

    pub fn get(&self) -> RefTypes<'_> {
        match self {
            Self::NoFile => RefTypes::NoFile,
            Self::Absent => RefTypes::Absent,
            Self::List(l) => RefTypes::List(l),
        }
    }
}

/// The widening `(double)f` of x86-64 (a NaN's quiet bit set, its payload kept).
pub fn widen(bits: u32) -> u64 {
    let f = f32::from_bits(bits);
    if f.is_nan() {
        let sign = u64::from(bits >> 31) << 63;
        let mant = u64::from(bits & 0x007f_ffff | 0x0040_0000) << 29;
        sign | 0x7ff0_0000_0000_0000 | mant
    } else {
        f64::from(f).to_bits()
    }
}

pub fn float(bits: u64) -> Json {
    json!({ "f": format!("{bits:016x}") })
}

/// `value` as the fixture writes the Python object UnityPy made.
pub fn enc(p: &Parsed, value: &Value) -> Json {
    match value {
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::F32(b) => float(widen(*b)),
        Value::F64(b) => float(*b),
        Value::Str(s) => json!({ "s": hex(s.as_bytes()) }),
        Value::Bytes(b) => json!({ "y": hex(b) }),
        Value::Prims(a) => Json::Array(match a {
            PrimArray::Bool(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::U8(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::I8(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::U16(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::I16(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::U32(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::I32(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::U64(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::I64(v) => v.iter().map(|x| json!(x)).collect(),
            PrimArray::F32(v) => v.iter().map(|x| float(widen(*x))).collect(),
            PrimArray::F64(v) => v.iter().map(|x| float(*x)).collect(),
        }),
        Value::List(v) => Json::Array(v.iter().map(|x| enc(p, x)).collect()),
        Value::Tuple(v) => json!({ "t": v.iter().map(|x| enc(p, x)).collect::<Vec<_>>() }),
        Value::Dict(m) => json!({ "d": dict(p, m) }),
    }
}

pub fn dict(p: &Parsed, m: &Map) -> Vec<Json> {
    p.entries(m)
        .map(|(k, v)| json!([String::from_utf8(k.to_vec()).unwrap(), enc(p, v)]))
        .collect()
}

/// A write's outcome as the fixture holds it, less the exception class.
#[allow(dead_code, reason = "walk_fixtures.rs compares the class too")]
pub fn write_out(
    p: &Parsed,
    tree: &Tree,
    endian: Endian,
    refs: RefTypes<'_>,
    rules: WriteRules,
) -> Json {
    match write_result(p, tree, endian, refs, rules) {
        Ok(bytes) => json!({ "hex": hex(&bytes) }),
        Err(_) => json!({ "err": true }),
    }
}

/// A write's bytes, or its error.
pub fn write_result(
    p: &Parsed,
    tree: &Tree,
    endian: Endian,
    refs: RefTypes<'_>,
    rules: WriteRules,
) -> Result<Vec<u8>, WriteError> {
    let limits = Limits::default();
    let ctx = WriteCtx {
        endian,
        ref_types: refs,
        rules,
        limits: &limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    walk::write::write(p, tree, &ctx, &mut out).map(|()| out.as_slice().to_vec())
}

pub fn endian(c: &Json) -> Endian {
    match c.get("endian").and_then(Json::as_str) {
        Some(">") => Endian::Big,
        _ => Endian::Little,
    }
}
