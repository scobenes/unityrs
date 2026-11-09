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
//! writer (against the case's `write_ref_types` when it names another file,
//! then under both write rules); `every_case_is_replayed` replays them all,
//! so a case no rule test names is still checked. A write UnityPy raised in
//! must fail with an error whose [`WriteError::exceptions`] name the class
//! it raised; a read's failure is compared by its kind only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList};
use unityrs_core::walk::{
    self, ParseError, ReadCtx, ReadError, ReadErrorKind, ReadOptions, RefTypes, WriteError,
    WriteRules,
};
use unityrs_core::{Bytes, Parsed, PrimArray, PrimRun, RawStr, Value, ValueBuilder};

mod walk_common;

use walk_common::{Refs, compile, enc, endian, hex, load, nodes, unhex, write_result};

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

/// Each class UnityPy raised in a write that unityrs refused, by the
/// refusal ([`label`]): the classes it stands for
/// ([`WriteError::exceptions`]) and those UnityPy raised.
type Raised = BTreeMap<String, (&'static [&'static str], BTreeSet<String>)>;

/// A refusal's variant, and for a type mismatch what the node takes, which
/// is what [`WriteError::exceptions`] goes by.
fn label(e: &WriteError) -> String {
    match e {
        WriteError::TypeMismatch { expected, .. } => format!("TypeMismatch: {expected}"),
        e => format!("{e:?}")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// A write against UnityPy's (`{"hex": ...}` or `{"err": class}`): `None`
/// when it wrote the same bytes, or failed with an error that stands for
/// the class UnityPy raised, which `raised` records.
fn write_differs(
    want: &Json,
    have: &Result<Vec<u8>, WriteError>,
    raised: &mut Raised,
) -> Option<String> {
    match (want["err"].as_str(), have) {
        (None, Ok(b)) if want["hex"].as_str() == Some(hex(b).as_str()) => None,
        (Some(class), Err(e)) if e.exceptions().contains(&class) => {
            raised
                .entry(label(e))
                .or_insert_with(|| (e.exceptions(), BTreeSet::new()))
                .1
                .insert(class.to_owned());
            None
        }
        (_, Ok(b)) => Some(format!("want {want}\n  have {{\"hex\":\"{}\"}}", hex(b))),
        (_, Err(e)) => Some(format!(
            "want {want}\n  have {e:?}, which stands for {:?}",
            e.exceptions()
        )),
    }
}

/// Replays one case through both readers and the writer; the mismatches.
fn replay(c: &Json, raised: &mut Raised) -> Vec<String> {
    let name = c["name"].as_str().unwrap();
    let tree = compile(&c["nodes"]);
    let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
    let start = c.get("start").and_then(Json::as_i64).unwrap_or(0);
    let refs = Refs::of(c.get("ref_types"));
    // The file each value is written against when it is not the one it was
    // read with (`write_ref_types`), where the mirror refuses it as
    // UnityPy's writer does.
    let other = c.get("write_ref_types").map(|r| Refs::of(Some(r)));
    let written_to = other.as_ref().unwrap_or(&refs);
    let rules: &[_] = if other.is_some() {
        &[
            ("write", WriteRules::UnityPy),
            ("mirror write", WriteRules::MirrorReader),
        ]
    } else {
        &[("write", WriteRules::UnityPy)]
    };
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
            let want = &c[format!("{reader}_write")];
            for &(what, rules) in rules {
                let have = write_result(&p, &tree, endian(c), written_to.get(), rules);
                if let Some(d) = write_differs(want, &have, raised) {
                    bad.push(format!("{name} [{reader} {what}]\n  {d}"));
                }
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
            bad.extend(replay(c, &mut Raised::new()));
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
fn r02_every_read_bounds_checked() {
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
fn w1_dispatch_type_name_pair_ref_object_array_child_class() {
    replay_prefix("w1_");
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
                    let _ = write_result(p, &tree, endian(c), refs.get(), rules);
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
    let bad = value_cases(&mut Raised::new());
    assert!(
        bad.is_empty(),
        "{} cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

fn value_cases(raised: &mut Raised) -> Vec<String> {
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
        let refs = RefTypes::List(&refs);
        let d = match parsed {
            None if c["l3"].is_null() => None,
            None => Some(format!("want {}\n  have no value", c["l3"])),
            Some(p) => {
                let have = write_result(p, &tree, endian(c), refs, WriteRules::UnityPy);
                write_differs(&c["l3"], &have, raised)
            }
        };
        if let Some(d) = d {
            bad.push(format!("{name} L3\n  {d}"));
        }
    }
    bad
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
            [1, "int", "x", 0],
            [1, "int", "type", 0]
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
/// Python's operations do (`tools/fixtures/walk_write.py`).
#[test]
fn w2_every_node_kind_from_every_value_variant() {
    let bad = write_matrix(&mut Raised::new());
    assert!(
        bad.is_empty(),
        "{} cases differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

fn write_matrix(raised: &mut Raised) -> Vec<String> {
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
        let have = write_result(
            &parsed,
            &tree,
            Endian::Little,
            RefTypes::List(&refs),
            WriteRules::UnityPy,
        );
        if let Some(d) = write_differs(&case[2], &have, raised) {
            bad.push(format!("{kname} <- {vname}\n  {d}"));
        }
    }
    bad
}

/// Review round 16: every write UnityPy raised in is refused with an error
/// that stands for the class it raised ([`WriteError::exceptions`]), and
/// each class an error names is one UnityPy raised there: over every write
/// recorded (the cases', the value cases' and W2's), the classes each
/// refusal (its variant, and for a type mismatch what the node takes)
/// stands for are exactly those UnityPy raised where unityrs refused so.
/// Every refusal that names a class is met but `NoTypeTree` (an object with
/// no tree, which `src/walk/write/tests.rs` pins to the read side's class);
/// the rest name none: a bound, a refused charge, a tree that does not
/// compile (the same file) and a dunder attribute (`PARITY.md` section 19.3;
/// `src/walk/write/work_tests.rs`). Before, a write's
/// failure was compared as a raise only, so a writer that raised
/// `ValueError` for `KeyError` passed; and until fix pass 17's review, a
/// file below format 20 (no `ref_types` attribute: `AttributeError`) was
/// refused as no file is (`ValueError`), which no recording met.
#[test]
fn write_errors_stand_for_the_classes_unitypy_raised() {
    let mut raised = Raised::new();
    let mut bad = Vec::new();
    for c in load("cases.json").as_array().unwrap() {
        bad.extend(replay(c, &mut raised));
    }
    bad.extend(value_cases(&mut raised));
    bad.extend(write_matrix(&mut raised));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    for (error, (stands_for, seen)) in &raised {
        let named: BTreeSet<String> = stands_for.iter().map(|c| (*c).to_owned()).collect();
        assert_eq!(&named, seen, "{error} stands for {stands_for:?}");
    }
    let met: Vec<(&str, &[&str])> = raised.iter().map(|(e, (s, _))| (e.as_str(), *s)).collect();
    let attr: &[&str] = &["AttributeError"];
    let type_error: &[&str] = &["TypeError"];
    assert_eq!(
        met,
        [
            ("Malformed", &["IndexError"][..]),
            ("MissingKey", &["KeyError"]),
            ("NoRefTypes", &["ValueError"]),
            ("NoRefTypesAttribute", attr),
            ("OutOfRange", &["error", "OverflowError"]),
            ("RefTypeNotFound", &["ValueError"]),
            (
                "TypeMismatch: a dict or an object with class, ns and asm",
                attr
            ),
            ("TypeMismatch: a ref type with a tree", attr),
            (
                "TypeMismatch: a sequence of two",
                &["TypeError", "KeyError", "IndexError"]
            ),
            ("TypeMismatch: a sized iterable", type_error),
            ("TypeMismatch: an object with this attribute", attr),
            ("TypeMismatch: bytes", type_error),
            ("TypeMismatch: dict", type_error),
            ("TypeMismatch: float", &["error"]),
            ("TypeMismatch: int", &["error"]),
            ("TypeMismatch: str", attr),
        ],
        "the refusals the recordings meet"
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
