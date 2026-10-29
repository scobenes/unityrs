//! Package B's oracle fixtures (G4), replayed without Python: the typetree
//! readers and the writer against UnityPy 1.25.2 with UnityPyBoost.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_fixtures
//!
//! `tools/fixtures/walk.py` builds every input in code, runs UnityPyBoost's
//! `read_typetree`, the pure-Python `read_value` and `write_typetree` on it,
//! and records the outcomes under `tests/fixtures/walk/` (its module docs
//! give the encoding). A value is compared by its Python types: a `Value`
//! is encoded the way the script encodes the object UnityPy returned, so a
//! `Prims` run and a `List` of the same integers are the same Python list,
//! and an `F32` is the Python float its bits widen to.
//!
//! Each rule of plan 3.10 and 3.11 has a test here named after it
//! (`r09_...`, `w5_...`) that replays the cases whose names start with its
//! id, from both readers and, after each successful read, through the
//! writer; `every_case_is_replayed` replays them all, so a case no rule test
//! names is still checked.

use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ParseError, ReadCtx, ReadError, ReadErrorKind, ReadOptions, RefTypes, WriteCtx,
    WriteRules,
};
use unityrs_core::{
    Bytes, LimitKind, Map, Parsed, PrimArray, PrimRun, RawStr, Value, ValueBuilder,
};

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

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
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

fn compile(rows: &Json) -> Arc<Tree> {
    Tree::compile(nodes(rows), &Limits::default()).unwrap()
}

/// A case's ref types: `null` is no file, `"absent"` a file without
/// `ref_types`, a list the file's ref types.
enum Refs {
    NoFile,
    Absent,
    List(Vec<SerializedType>),
}

impl Refs {
    fn of(spec: Option<&Json>) -> Self {
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

    fn get(&self) -> RefTypes<'_> {
        match self {
            Self::NoFile => RefTypes::NoFile,
            Self::Absent => RefTypes::Absent,
            Self::List(l) => RefTypes::List(l),
        }
    }
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

fn float(bits: u64) -> Json {
    json!({ "f": format!("{bits:016x}") })
}

/// `value` as the fixture writes the Python object UnityPy made.
fn enc(p: &Parsed, value: &Value) -> Json {
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

fn dict(p: &Parsed, m: &Map) -> Vec<Json> {
    p.entries(m)
        .map(|(k, v)| json!([String::from_utf8(k.to_vec()).unwrap(), enc(p, v)]))
        .collect()
}

fn kind(k: ReadErrorKind) -> &'static str {
    match k {
        ReadErrorKind::OutOfBounds => "out_of_bounds",
        ReadErrorKind::NegativeLength => "negative_length",
        ReadErrorKind::RefType => "ref_type",
        ReadErrorKind::Other => "other",
    }
}

/// A read's outcome as the fixture holds it, less the exception class.
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

fn write_out(
    p: &Parsed,
    tree: &Tree,
    endian: Endian,
    refs: RefTypes<'_>,
    rules: WriteRules,
) -> Json {
    let limits = Limits::default();
    let ctx = WriteCtx {
        endian,
        ref_types: refs,
        rules,
        limits: &limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    match walk::write::write(p, tree, &ctx, &mut out) {
        Ok(()) => json!({ "hex": hex(out.as_slice()) }),
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

fn endian(c: &Json) -> Endian {
    match c.get("endian").and_then(Json::as_str) {
        Some(">") => Endian::Big,
        _ => Endian::Little,
    }
}

/// Replays one case through both readers and the writer; the mismatches.
fn replay(c: &Json) -> Vec<String> {
    let name = c["name"].as_str().unwrap();
    let tree = compile(&c["nodes"]);
    let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
    let start = c.get("start").and_then(Json::as_i64).unwrap_or(0);
    let refs = Refs::of(c.get("ref_types"));
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: endian(c),
        ref_types: refs.get(),
        options: &options,
        meter: &meter,
    };
    let mut bad = Vec::new();
    let object = data.slice(usize::try_from(start).unwrap()..);
    for (reader, got) in [
        ("boost", walk::boost::read(&object, &tree, &ctx)),
        (
            "python",
            walk::python::read(&data, i128::from(start), &tree, &ctx),
        ),
    ] {
        let want = expected(&c[reader]);
        let have = outcome(&got);
        if have != want {
            bad.push(format!("{name} [{reader}]\n  want {want}\n  have {have}"));
            continue;
        }
        if let Ok(p) = got {
            let want = expected_write(&c[format!("{reader}_write")]);
            let have = write_out(&p, &tree, endian(c), refs.get(), WriteRules::UnityPy);
            if have != want {
                bad.push(format!(
                    "{name} [{reader} write]\n  want {want}\n  have {have}"
                ));
            }
        }
    }
    bad
}

fn replay_prefix(prefix: &str) {
    let cases = load("cases.json");
    let mut n = 0;
    let mut bad = Vec::new();
    for c in cases.as_array().unwrap() {
        if c["name"].as_str().unwrap().starts_with(prefix) {
            n += 1;
            bad.extend(replay(c));
        }
    }
    assert!(n > 0, "no case starts with {prefix}");
    assert!(
        bad.is_empty(),
        "{} of {n} cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_case_is_replayed() {
    replay_prefix("");
}

#[test]
fn r01_kind_by_exact_match_pptr_prefix_else_class() {
    replay_prefix("r01_");
}

#[test]
fn r02_bounds_checked_even_for_zero_bytes() {
    replay_prefix("r02_");
}

#[test]
fn r03_align_from_object_start_unchecked() {
    replay_prefix("r03_");
}

#[test]
fn r04_bool_char_typeptr_filesize() {
    replay_prefix("r04_");
}

#[test]
fn r05_float_widened_snan_quieted() {
    replay_prefix("r05_");
}

#[test]
fn r06_string_always_aligned_negative_before_move() {
    replay_prefix("r06_");
}

#[test]
fn r07_typeless_data_aligned_by_own_flag() {
    replay_prefix("r07_");
}

#[test]
fn r08_pair_needs_two_children() {
    replay_prefix("r08_");
}

#[test]
fn r09_own_kind_first_then_array_child() {
    replay_prefix("r09_");
}

#[test]
fn r10_runs_align_once_pair_arity_before_elements() {
    replay_prefix("r10_");
}

#[test]
fn r11_other_elements_one_by_one() {
    replay_prefix("r11_");
}

#[test]
fn r12_repeated_names_first_position_last_value() {
    replay_prefix("r12_");
}

#[test]
fn r13_registry_scope() {
    replay_prefix("r13_");
}

#[test]
fn r14_referenced_object_resolution_order() {
    replay_prefix("r14_");
}

#[test]
fn r15_align_after_flagged_node() {
    replay_prefix("r15_");
}

#[test]
fn r16_structural_error_kinds() {
    replay_prefix("r16_");
}

#[test]
fn pure_python_reader_list() {
    replay_prefix("py_");
}

#[test]
fn w3_strings_always_align() {
    replay_prefix("w3_");
}

#[test]
fn w4_typeless_data_aligns_when_flagged() {
    replay_prefix("w4_");
}

#[test]
fn w5_array_elements_align_each() {
    replay_prefix("w5_");
}

#[test]
fn w6_class_writes_each_child_registry_scope() {
    replay_prefix("w6_");
}

#[test]
fn w7_referenced_object_write_failures() {
    replay_prefix("w7_");
}

/// `WriteRules::MirrorReader` (plan 3.11): each value UnityPyBoost reads
/// from a tree that repeats no name (a repeated name keeps its last value
/// only) writes, under the mirror, to bytes the reader reads back to the
/// same value and the same `bytes_read`. Under `WriteRules::UnityPy` some do
/// not: a flagged run's elements each pad, and an empty-class
/// `ReferencedObject` fails, so the mirror is what makes the round trip.
#[test]
fn mirror_reader_round_trips_what_the_reader_reads() {
    let cases = load("cases.json");
    let (mut n, mut unitypy_differs) = (0, 0);
    let mut bad = Vec::new();
    for c in cases.as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let tree = compile(&c["nodes"]);
        let unique = (0..tree.list().len()).all(|i| {
            let id = NodeId(u32::try_from(i).unwrap());
            tree.info(id).is_none_or(|x| x.key == id)
        });
        let refs = Refs::of(c.get("ref_types"));
        // A ref type's own tree may repeat names too.
        let ref_unique = match &refs {
            Refs::List(l) => l.iter().all(|r| match r.tree(&Limits::default()) {
                Some(Ok(t)) => (0..t.list().len()).all(|i| {
                    let id = NodeId(u32::try_from(i).unwrap());
                    t.info(id).is_none_or(|x| x.key == id)
                }),
                _ => true,
            }),
            _ => true,
        };
        let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        let start = usize::try_from(c.get("start").and_then(Json::as_i64).unwrap_or(0)).unwrap();
        let options = ReadOptions::default();
        let meter = Meter::unlimited();
        let ctx = ReadCtx {
            endian: endian(c),
            ref_types: refs.get(),
            options: &options,
            meter: &meter,
        };
        let Ok(first) = walk::boost::read(&data.slice(start..), &tree, &ctx) else {
            continue;
        };
        if !unique || !ref_unique {
            continue;
        }
        n += 1;
        for rules in [WriteRules::MirrorReader, WriteRules::UnityPy] {
            let written = write_out(&first, &tree, endian(c), refs.get(), rules);
            let again = written["hex"]
                .as_str()
                .map(|h| walk::boost::read(&Bytes::from(unhex(h)), &tree, &ctx));
            let same = match &again {
                Some(Ok(p)) => {
                    enc(p, p.value()) == enc(&first, first.value())
                        && p.bytes_read == first.bytes_read
                        && i64::try_from(unhex(written["hex"].as_str().unwrap()).len()).unwrap()
                            == first.bytes_read
                }
                _ => false,
            };
            match rules {
                WriteRules::MirrorReader if !same => bad.push(format!("{name}: {written}")),
                WriteRules::UnityPy if !same => unitypy_differs += 1,
                _ => {}
            }
        }
    }
    assert!(n > 80, "only {n} cases read");
    assert!(
        bad.is_empty(),
        "{} of {n} do not round-trip:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(unitypy_differs > 0, "no case needs the mirror");
}

/// Seeded no-panic runs over the recorded cases with their bytes mutated
/// (bits flipped, or cut short): 30 per case, through both readers and,
/// after each successful read, the writer under both rules.
#[test]
fn mutated_cases_never_panic() {
    let cases = load("cases.json");
    let mut rng = unityrs_testkit::Rng::new(0xb0b5_5eed);
    let mut ok = 0;
    for c in cases.as_array().unwrap() {
        let tree = compile(&c["nodes"]);
        let refs = Refs::of(c.get("ref_types"));
        let original = unhex(c["hex"].as_str().unwrap());
        for _ in 0..30 {
            let mut data = original.clone();
            if rng.below(3) == 0 {
                rng.truncate(&mut data);
            } else {
                let flips = 1 + rng.below(4);
                rng.flip_bits(&mut data, flips);
            }
            let data = Bytes::from(data);
            // Bounds a flipped count's work (each limit has its own test).
            let mut limits = Limits::default();
            limits.max_zero_size_elements = 256;
            limits.max_values_per_object = 1 << 14;
            let options = ReadOptions {
                limits,
                ..ReadOptions::default()
            };
            let meter = Meter::unlimited();
            let ctx = ReadCtx {
                endian: endian(c),
                ref_types: refs.get(),
                options: &options,
                meter: &meter,
            };
            let start = if rng.below(4) == 0 {
                rng.below(data.len() + 1)
            } else {
                0
            };
            let reads = [
                walk::boost::read(&data.slice(start..), &tree, &ctx),
                walk::python::read(&data, i128::try_from(start).unwrap(), &tree, &ctx),
            ];
            for p in reads.iter().flatten() {
                ok += 1;
                for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
                    let _ = write_out(p, &tree, endian(c), refs.get(), rules);
                }
            }
        }
    }
    assert!(ok > 1000, "only {ok} mutated reads succeeded");
}

/// G2: the spike's 32 value cases, read as `parse_as_dict` reads them
/// (`read_typetree` with `check_read`), and re-saved (L2 and L3).
#[test]
fn value_cases_l2_and_l3() {
    let cases = load("value_cases.json");
    let cases = cases.as_array().unwrap();
    assert_eq!(cases.len(), 32);
    let mut bad = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let rows: Vec<Json> = c["nodes"].as_array().unwrap().clone();
        let tree = compile(&Json::Array(rows));
        let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        let options = ReadOptions::default();
        let meter = Meter::unlimited();
        let refs: [SerializedType; 0] = [];
        let ctx = ReadCtx {
            endian: endian(c),
            ref_types: RefTypes::List(&refs),
            options: &options,
            meter: &meter,
        };
        let size = u32::try_from(data.len()).unwrap();
        let got = walk::read_typetree(&data, 0, size, &tree, &ctx, true);
        let (l2, parsed) = match &got {
            Ok(p) => (
                json!({"kind": null, "bytes_read": null, "value": enc(p, p.value())}),
                Some(p),
            ),
            Err(ParseError::CheckRead { parsed, .. }) => (
                json!({"kind": "check_read", "bytes_read": parsed.bytes_read, "value": enc(parsed, parsed.value())}),
                None,
            ),
            Err(ParseError::Read(e)) => (
                json!({"kind": kind(e.kind), "bytes_read": null, "value": null}),
                None,
            ),
            Err(e) => panic!("{name}: {e}"),
        };
        if l2 != c["l2"] {
            bad.push(format!("{name} L2\n  want {}\n  have {l2}", c["l2"]));
            continue;
        }
        let l3 = parsed.map_or(Json::Null, |p| {
            write_out(
                p,
                &tree,
                endian(c),
                RefTypes::List(&refs),
                WriteRules::UnityPy,
            )
        });
        let want = if c["l3"].is_null() {
            Json::Null
        } else {
            expected_write(&c["l3"])
        };
        if l3 != want {
            bad.push(format!("{name} L3\n  want {want}\n  have {l3}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Builds a W2 value spec into `items` (a list's or a tuple's) or returns it.
struct Specs {
    builder: ValueBuilder,
    aux: Arc<NodeList>,
    aux_ix: u32,
}

impl Specs {
    fn new(root: Arc<NodeList>) -> Self {
        let mut builder = ValueBuilder::new(&Meter::unlimited(), root).unwrap();
        let aux = nodes(&json!([
            [0, "Aux", "Aux", 0],
            [1, "int", "a", 0],
            [1, "int", "real", 0],
            [1, "int", "x", 0]
        ]));
        let aux_ix = builder.list_index(&aux).unwrap();
        Self {
            builder,
            aux,
            aux_ix,
        }
    }

    fn key(&self, name: &str) -> NodeId {
        (0..self.aux.len())
            .map(|i| NodeId(u32::try_from(i).unwrap()))
            .find(|k| self.aux.name(*k) == name.as_bytes())
            .unwrap()
    }

    fn build(&self, spec: &Json) -> unityrs_core::Built {
        let b = &self.builder;
        let (tag, v) = spec.as_object().unwrap().iter().next().unwrap();
        let scalar = |v: Value| b.scalar(v).unwrap();
        match tag.as_str() {
            "bool" => scalar(Value::Bool(v.as_bool().unwrap())),
            "int" => scalar(Value::Int(v.as_i64().unwrap())),
            "uint" => scalar(Value::UInt(v.as_u64().unwrap())),
            "f32" => scalar(Value::F32(
                u32::from_str_radix(v.as_str().unwrap(), 16).unwrap(),
            )),
            "f64" => scalar(Value::F64(
                u64::from_str_radix(v.as_str().unwrap(), 16).unwrap(),
            )),
            "str" => scalar(Value::Str(RawStr::from(unhex(v.as_str().unwrap())))),
            "bytes" => scalar(Value::Bytes(Bytes::from(unhex(v.as_str().unwrap())))),
            "prims_u8" => scalar(Value::Prims(PrimArray::U8(Bytes::from(unhex(
                v.as_str().unwrap(),
            ))))),
            "prims_i32" => {
                let mut items = b.vec();
                for x in v.as_array().unwrap() {
                    items
                        .push(i32::try_from(x.as_i64().unwrap()).unwrap())
                        .unwrap();
                }
                b.prims(PrimRun::I32(items)).unwrap()
            }
            "prims_f32" => {
                let mut items = b.vec();
                for x in v.as_array().unwrap() {
                    items
                        .push(u32::from_str_radix(x.as_str().unwrap(), 16).unwrap())
                        .unwrap();
                }
                b.prims(PrimRun::F32(items)).unwrap()
            }
            "prims_bool" => {
                let mut items = b.vec();
                for x in v.as_array().unwrap() {
                    items.push(x.as_bool().unwrap()).unwrap();
                }
                b.prims(PrimRun::Bool(items)).unwrap()
            }
            "list" | "tuple" => {
                let mut items = b.vec();
                for x in v.as_array().unwrap() {
                    items.push_built(self.build(x)).unwrap();
                }
                if tag == "list" {
                    b.list(items)
                } else {
                    b.tuple(items).unwrap()
                }
            }
            "dict" => {
                let mut entries = b.vec();
                for e in v.as_array().unwrap() {
                    let key = self.key(e[0].as_str().unwrap());
                    entries.push_entry(key, self.build(&e[1])).unwrap();
                }
                b.map(self.aux_ix, entries)
            }
            other => panic!("spec {other}"),
        }
    }
}

/// W2: every node kind written from every value variant, duck typing as
/// Python's operations do.
#[test]
fn w2_every_node_kind_from_every_value_variant() {
    let m = load("write_matrix.json");
    let mut bad = Vec::new();
    for case in m["cases"].as_array().unwrap() {
        let (kname, vname) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        let tree = compile(&m["nodes"][kname]);
        let specs = Specs::new(tree.list().clone());
        let field = specs.build(&m["values"][vname]);
        let b = &specs.builder;
        let mut entries = b.vec();
        entries.push_entry(NodeId(1), field).unwrap();
        let root = b.map(0, entries);
        let Specs { builder, .. } = specs;
        let parsed = builder.finish(root, 0);
        let refs: [SerializedType; 0] = [];
        let have = write_out(
            &parsed,
            &tree,
            Endian::Little,
            RefTypes::List(&refs),
            WriteRules::UnityPy,
        );
        let want = expected_write(&case[2]);
        if have != want {
            bad.push(format!("{kname} <- {vname}\n  want {want}\n  have {have}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The writer's model of `getattr` on each Python type a value becomes
/// equals Python 3.11's non-dunder attributes (W2).
#[test]
fn w2_python_attributes_match_the_oracle() {
    let attrs = load("py_attrs.json");
    for (ty, names) in attrs.as_object().unwrap() {
        let want: Vec<(String, String)> = names
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a[0].as_str().unwrap().to_owned(),
                    a[1].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let have: Vec<(String, String)> = walk::write::python_attributes(ty)
            .iter()
            .map(|(n, k)| ((*n).to_owned(), (*k).to_owned()))
            .collect();
        assert_eq!(have, want, "{ty}");
    }
}
