//! Primitive runs and `Limits::max_values_per_object` (plan 3.10,
//! "Bounds"; `walk::Work`'s docs), at the defaults:
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_runs
//!
//! Review round 9 found a well-formed object refused where UnityPy reads
//! it: a `Mesh` whose `vector<UInt8>` held 2^26 + 1 bytes, each element
//! counted as a value against the default bound of 2^26, on read and on
//! re-save. UnityPyBoost's reader holds a primitive run as one value whose
//! elements are the object's bytes, which its position passes once, so the
//! run counts as its vector's one value and its bytes as the open's work;
//! the writer counts no primitive against the bound outside a repeated
//! name. `tests/fixtures/walk/runs.json` is recorded by
//! `tools/fixtures/walk.py` from the cases of `tools/fixtures/walk_runs.py`
//! (its module docs give the recipe and the encoding): a `vector<UInt8>`
//! and a `vector<float>` of 2^26 + 1 elements, each read by UnityPyBoost
//! and re-saved; and (review round 10) a vector of 2^26 + 1 elements whose
//! type name is `int\0x`, which UnityPyBoost reads as a run of `int`s and
//! UnityPy's writer writes as classes with no children, which write
//! nothing: the writer counted each such class against the bound. The
//! pure-Python reader still counts each element of a run it reads, since a
//! negative `TypelessData` length lets it read the same bytes again and
//! again (the last test).

use std::sync::Arc;

use flate2::Crc;
use serde_json::{Value as Json, json};
use unityrs_core::budget::{DecodeBuf, Reservation, Unbounded};
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ReadCtx, ReadDetail, ReadError, ReadOptions, RefTypes, WriteCtx, WriteRules,
};
use unityrs_core::{Bytes, LimitKind, Parsed, PrimArray, Value};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/walk/runs.json");

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

fn crc(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut c = Crc::new();
    let mut buf = [0_u8; 4096];
    let mut n = 0;
    for b in bytes {
        buf[n] = b;
        n += 1;
        if n == buf.len() {
            c.update(&buf);
            n = 0;
        }
    }
    c.update(&buf[..n]);
    c.sum()
}

/// The widening `(double)f` of x86-64 (a NaN's quiet bit set, its payload kept).
fn widen(bits: u32) -> u64 {
    let f = f32::from_bits(bits);
    if f.is_nan() {
        let sign = u64::from(bits >> 31) << 63;
        let mant = u64::from(bits & 0x007f_ffff | 0x0040_0000) << 29;
        sign | 0x7ff0_0000_0000_0000 | mant
    } else {
        f64::from(f).to_bits()
    }
}

/// The vector as `walk_runs.py` summarises the list UnityPy made.
fn summary(v: &Value) -> Json {
    match v {
        Value::Prims(PrimArray::U8(b)) => {
            json!({ "n": b.len(), "u8_crc32": crc(b.iter().copied()) })
        }
        Value::Prims(PrimArray::I32(v)) => json!({
            "n": v.len(),
            "i32_crc32": crc(v.iter().flat_map(|x| x.to_le_bytes())),
        }),
        Value::Prims(PrimArray::F32(f)) => json!({
            "n": f.len(),
            "f64_crc32": crc(f.iter().flat_map(|b| widen(*b).to_le_bytes())),
        }),
        other => panic!("no case reads {other:?}"),
    }
}

/// The value as `walk_runs.py` writes it: `m_Name`, then the vector's summary.
fn outcome(parsed: &Parsed) -> Json {
    let Value::Dict(dict) = parsed.value() else {
        panic!("not a dict: {:?}", parsed.value());
    };
    let mut entries = Vec::new();
    for (key, value) in parsed.entries(dict) {
        let key = String::from_utf8(key.to_vec()).unwrap();
        let enc = match value {
            Value::Str(s) => json!({ "s": hex(s.as_bytes()) }),
            other => summary(other),
        };
        entries.push(json!([key, enc]));
    }
    json!({ "ok": { "d": entries }, "pos": parsed.bytes_read })
}

/// The input's meter at the defaults, as `Environment` makes one.
fn meter() -> Arc<Meter> {
    Meter::for_input(&Limits::default(), Arc::new(Unbounded), Reservation::Input)
}

fn read(
    reader: &str,
    data: &Bytes,
    tree: &Arc<Tree>,
    meter: &Arc<Meter>,
) -> Result<Parsed, ReadError> {
    let options = EnvOptions::unitypy().context.read_options();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter,
    };
    if reader == "boost" {
        walk::boost::read(data, tree, &ctx)
    } else {
        walk::python::read(data, 0, tree, &ctx)
    }
}

/// Review round 9's HIGH: each run of 2^26 + 1 elements, past the default
/// `max_values_per_object`, reads through UnityPyBoost's reader at the
/// defaults as UnityPy reads it, and re-saves as UnityPy re-saves it
/// (review round 10: the `int\0x` run's re-save was refused). The
/// pure-Python reader, which reads an object only when its `byte_size` is
/// 0 (so such an object fails `check_read` in UnityPy), still refuses a
/// run; it reads `int\0x` elements as classes, not as a run
/// (`tests/fixtures/walk/cases.json`'s `w1_nul_type_name_run`).
#[test]
fn runs_past_the_values_bound_read_and_resave_as_unitypy_at_the_defaults() {
    let limits = EnvOptions::unitypy().context.limits;
    assert_eq!(limits, Limits::default());
    let cases: Json = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let cases = cases.as_array().unwrap();
    assert_eq!(
        cases.len(),
        3,
        "a vector<UInt8>, a vector<float> and a vector<int\\0x>"
    );
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let tree = compile(&c["nodes"]);
        let data = data(&c["runs"]);
        let meter = meter();
        let p = read("boost", &data, &tree, &meter).unwrap_or_else(|e| panic!("{name}: {e}"));
        let have = outcome(&p);
        assert_eq!(have, c["boost"], "{name}: the read");
        let n = have["ok"]["d"][1][1]["n"].as_u64().unwrap();
        assert!(n > limits.max_values_per_object, "{name}: {n} elements");
        let wctx = WriteCtx {
            endian: Endian::Little,
            ref_types: RefTypes::NoFile,
            rules: WriteRules::UnityPy,
            limits: &limits,
        };
        let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
        walk::write::write(&p, &tree, &wctx, &mut out).unwrap_or_else(|e| panic!("{name}: {e}"));
        let written = json!({ "n": out.len(), "crc32": crc(out.as_slice().iter().copied()) });
        assert_eq!(written, c["boost_write"], "{name}: the re-save");
        drop((out, p));
        let elem = c["nodes"].as_array().unwrap().last().unwrap()[1].as_str();
        if elem.unwrap().contains('\0') {
            continue;
        }
        match read("python", &data, &tree, &meter) {
            Err(ReadError {
                detail: ReadDetail::Limit(l),
                ..
            }) if l.kind() == LimitKind::ValuesPerObject => {}
            other => panic!(
                "{name}: the pure-Python reader: {:?}",
                other.map(|p| p.bytes_read)
            ),
        }
    }
}

/// Why the pure-Python reader counts each element of a run (`walk::Work`'s
/// docs): its bytes do not bound them. `Base { vector<E> v }`, each `E` a
/// `vector<UInt8>` of 256 bytes and a `TypelessData` of length -264, which
/// takes the reader back to the element's start: 2^12 elements read the
/// same 256 bytes 2^12 times, 2^20 bytes from a file of 268. Counted one by
/// one they pass a bound of 2^16 and the read is refused there; the
/// elements' other values (each `E`, its vector and its `TypelessData`),
/// 3 x 2^12, stay under it. UnityPyBoost refuses the negative length.
#[test]
fn the_pure_python_reader_counts_each_element_it_reads_again() {
    const ELEMENTS: i32 = 1 << 12;
    const BYTES: i32 = 256;
    let rows = [
        (0, "Base", "Base"),
        (1, "vector", "v"),
        (2, "Array", "Array"),
        (3, "int", "size"),
        (3, "E", "data"),
        (4, "vector", "b"),
        (5, "Array", "Array"),
        (6, "int", "size"),
        (6, "UInt8", "data"),
        (4, "TypelessData", "back"),
    ];
    let rows = rows
        .iter()
        .map(|&(level, ty, name)| NodeRow::new(level, ty, name))
        .collect();
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let mut d = ELEMENTS.to_le_bytes().to_vec();
    d.extend(BYTES.to_le_bytes());
    d.extend(0..=u8::MAX);
    d.extend((-(BYTES + 8)).to_le_bytes());
    let data = Bytes::from(d);
    assert_eq!(data.len(), 4 + 4 + 256 + 4);
    let bound = 1_u64 << 16;
    assert!(3 * u64::from(ELEMENTS.unsigned_abs()) < bound);
    assert!(u64::from((ELEMENTS * BYTES).unsigned_abs()) > bound);
    let mut limits = Limits::default();
    limits.max_values_per_object = bound;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter: &meter,
    };
    match walk::python::read(&data, 0, &tree, &ctx) {
        Err(ReadError {
            detail: ReadDetail::Limit(l),
            ..
        }) => assert_eq!(
            (l.kind(), l.value(), l.max()),
            (LimitKind::ValuesPerObject, bound + 1, bound)
        ),
        other => panic!("not refused: {:?}", other.map(|p| p.bytes_read)),
    }
    // With the bound lifted it reads every element, each back at its start.
    limits.max_values_per_object = u64::MAX;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let ctx = ReadCtx {
        options: &options,
        ..ctx
    };
    let p = walk::python::read(&data, 0, &tree, &ctx).unwrap();
    assert_eq!(p.bytes_read, 4);
    let Value::Dict(m) = p.value() else {
        panic!("{:?}", p.value())
    };
    let Some((_, Value::List(items))) = m.entries().first() else {
        panic!("{m:?}")
    };
    assert_eq!(items.len(), 1 << 12);
}
