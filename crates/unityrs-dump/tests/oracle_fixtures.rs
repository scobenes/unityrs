//! Replays `tools/fixtures/dump.py`'s oracle fixtures without Python (plan
//! section 6, G4): `canonical.py`'s encodings of synthetic values and names,
//! Python's decimal form of packed integers, `unitypy_dump.py`'s input
//! selection, what its run makes of an index's pins and shapes, and
//! `fnmatch.fnmatchcase`, `dump_records.py`'s `error_kind` on
//! `check_read` messages and its `first_difference`, and the oracle's records
//! of `SerializedFile`s the fixture script built (`records.json`).

mod common;

use std::sync::Arc;

use common::{Tmp, fixture, parsed, s, unhex};
use serde_json::Value as Json;
use unityrs_core::FileContext;
use unityrs_core::walk::L2Kind;
use unityrs_dump::encode::encode;
use unityrs_dump::input::check_read_kind;
use unityrs_dump::json::{limbs_to_decimal, write_name};
use unityrs_dump::record::first_difference;
use unityrs_dump::run::{RunError, RunOptions, Summary, run};
use unityrs_dump::select::{PyStr, Selection, collect, fnmatchcase};
use unityrs_dump::{DumpOptions, dump_input};

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

// --------------------------------------------------------------- selection

#[test]
fn globs_match_as_fnmatchcase() {
    let f = fixture("select.json");
    let mut failed = Vec::new();
    for c in f["fnmatch"].as_array().unwrap() {
        let (name, pat) = (unhex(s(&c["name"])), unhex(s(&c["pattern"])));
        let got = fnmatchcase(&name, &pat).unwrap();
        if got != c["match"].as_bool().unwrap() {
            failed.push(format!(
                "{:?} ~ {:?}: got {got}",
                String::from_utf8_lossy(&name),
                String::from_utf8_lossy(&pat)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

fn make_tree(f: &Json) -> Tmp {
    use std::os::unix::ffi::OsStrExt;
    let tmp = Tmp::new("select");
    let root = tmp.0.clone();
    for e in f["tree"].as_array().unwrap() {
        let rel = unhex(s(&e[0]));
        let path = root.join(std::ffi::OsStr::from_bytes(&rel));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match s(&e[1]) {
            "file" => std::fs::write(&path, &rel).unwrap(),
            "dir" => std::fs::create_dir_all(&path).unwrap(),
            "symlink" => {
                let target = unhex(s(&e[2]));
                std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&target), &path).unwrap();
            }
            other => panic!("{other}"),
        }
    }
    std::fs::write(
        root.join("corpus/index.json"),
        serde_json::to_string(&f["index"]).unwrap(),
    )
    .unwrap();
    tmp
}

#[test]
fn inputs_are_selected_named_and_ordered_as_the_oracle_selects_them() {
    use std::os::unix::ffi::OsStrExt;
    let f = fixture("select.json");
    let tmp = make_tree(&f);
    let root = &tmp.0;
    let mut failed = Vec::new();
    for c in f["selections"].as_array().unwrap() {
        let args = &c["args"];
        let strings = |k: &str| -> Option<Vec<PyStr>> {
            args.get(k).map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| PyStr::from(s(x)))
                    .collect()
            })
        };
        let mut sel = Selection {
            inputs: vec![root.join(s(&c["input"]))],
            index: args.get("index").and_then(Json::as_bool).unwrap_or(false),
            builds: strings("build"),
            names: args
                .get("name_hex")
                .map(|v| v.as_array().unwrap().iter().map(|x| unhex(s(x))).collect())
                .unwrap_or_default(),
            sample: args.get("sample").and_then(Json::as_i64),
            limit: args.get("limit").and_then(Json::as_i64),
            ..Selection::default()
        };
        if let Some(k) = strings("kind") {
            sel.kinds = k;
        }
        let got: Vec<(String, String, Option<String>)> = collect(&sel)
            .unwrap()
            .into_iter()
            .map(|i| {
                let rel = i.path.strip_prefix(root).unwrap();
                (
                    unityrs_dump::json::hex(&i.name),
                    unityrs_dump::json::hex(rel.as_os_str().as_bytes()),
                    // The index pins only with strings: the run's reading
                    // of every other value is `pins_are_tested_as_the_oracle_tests_them`.
                    i.sha256.map(|p| p.to_string()),
                )
            })
            .collect();
        let want: Vec<(String, String, Option<String>)> = c["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    s(&r[0]).to_owned(),
                    s(&r[1]).to_owned(),
                    r[2].as_str().map(str::to_owned),
                )
            })
            .collect();
        if got != want {
            failed.push(format!(
                "{}:\n  got  {got:?}\n  want {want:?}",
                s(&c["name"])
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

// ------------------------------------------------------------- index pins

/// Runs the dump on the index at `index`, a file per entry beside it, as
/// `unitypy_dump.py --index` would be run.
fn run_index(index: &std::path::Path, allow: bool) -> Result<Summary, RunError> {
    run(&RunOptions {
        selection: Selection {
            inputs: vec![index.to_path_buf()],
            index: true,
            ..Selection::default()
        },
        output: index.with_file_name("out.jsonl"),
        dump: DumpOptions::default(),
        stats: None,
        quiet: true,
        allow_index_mismatch: allow,
    })
}

/// What an index entry's `sha256` pins, a value of every JSON kind and a
/// repeated name's several values, as the oracle's run read it
/// (`select.json` "pins": its own count of index mismatches, from its
/// warnings): with `--allow-index-mismatch` the run counts as many, and
/// without it a case the oracle counted any for fails the run.
#[test]
fn pins_are_tested_as_the_oracle_tests_them() {
    let f = fixture("select.json");
    let cases = f["pins"].as_array().unwrap();
    assert!(cases.len() >= 20, "{} pin cases", cases.len());
    let tmp = Tmp::new("pins");
    let mut failed = Vec::new();
    for c in cases {
        let name = s(&c["name"]);
        std::fs::create_dir_all(tmp.0.join(name)).unwrap();
        std::fs::write(tmp.0.join(name).join("f"), unhex(s(&c["data"]))).unwrap();
        let entries: Vec<&str> = c["entries"].as_array().unwrap().iter().map(s).collect();
        let index = tmp.0.join(format!("{name}.json"));
        std::fs::write(&index, format!(r#"{{"files":[{}]}}"#, entries.join(","))).unwrap();
        let want = c["mismatches"].as_u64().unwrap();
        let allowed = run_index(&index, true).map(|r| r.index_sha256_mismatch);
        let refused = run_index(&index, false).is_err();
        if allowed.as_ref().ok() != Some(&want) || refused != (want > 0) {
            failed.push(format!(
                "{name}: got {allowed:?} (refused without --allow-index-mismatch: {refused}), want {want}"
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// An index whose `files` is no list of objects (`select.json` "shapes"):
/// the run's exit status is the oracle's, whether `for e in
/// index["files"]` iterates nothing (no inputs matched: 2) or what it
/// iterates has no `get` (1).
#[test]
fn an_index_of_every_shape_exits_as_the_oracle_does() {
    let f = fixture("select.json");
    let cases = f["shapes"].as_array().unwrap();
    let tmp = Tmp::new("index-shapes");
    std::fs::write(tmp.0.join("f"), b"f").unwrap();
    let mut failed = Vec::new();
    for c in cases {
        let name = s(&c["name"]);
        let index = tmp.0.join(format!("{name}.json"));
        std::fs::write(&index, s(&c["index"])).unwrap();
        let got = run_index(&index, true).map_or_else(|e| e.status(), |_| 0);
        let want = c["status"].as_i64().unwrap();
        if i64::from(got) != want {
            failed.push(format!("{name}: exit {got}, want {want}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

// ---------------------------------------------------------------- L2 kinds

#[test]
fn check_read_kinds_are_the_oracles() {
    let f = fixture("l2_kinds.json");
    let cases = f["cases"].as_array().unwrap();
    let mut negative_probe = false;
    for c in cases {
        let read: i64 = if let Some(r) = c.get("read") {
            r.as_i64().unwrap()
        } else {
            // A real read's message: "..., but only read N bytes".
            let m = s(&c["message"]);
            let n = m.rsplit("only read ").next().unwrap();
            n.trim_end_matches(" bytes").parse().unwrap()
        };
        negative_probe |= c.get("read").is_none() && read < 0;
        let (kind, bytes_read) = check_read_kind(read);
        let want = match s(&c["kind"]) {
            "check_read" => L2Kind::CheckRead,
            "other" => L2Kind::Other,
            k => panic!("{k}"),
        };
        assert_eq!(kind, want, "{c}");
        assert_eq!(bytes_read, c["bytes_read"].as_i64(), "{c}");
    }
    assert!(negative_probe, "the fixture holds a real negative read");
}

// ----------------------------------------------------------------- records

#[test]
fn first_differences_are_the_oracles() {
    let f = fixture("records.json");
    let cases = f["first_diff"].as_array().unwrap();
    assert!(cases.len() >= 6, "{} cases", cases.len());
    for c in cases {
        let (a, b) = (unhex(s(&c["a"])), unhex(s(&c["b"])));
        assert_eq!(
            first_difference(&a, &b),
            c["first_diff"].as_u64().unwrap(),
            "{c}"
        );
    }
}

/// The oracle's records of a fixture input, by name, and its bytes.
fn oracle_input(name: &str) -> (Vec<u8>, Vec<Json>) {
    let f = fixture("records.json");
    let c = f["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| s(&c["name"]) == name)
        .unwrap_or_else(|| panic!("no input {name}"));
    let records = c["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| serde_json::from_str(s(l)).unwrap())
        .collect();
    (unhex(s(&c["input"])), records)
}

/// The dump's records of `data` named `name` under `ctx`.
fn records_of(name: &str, data: &[u8], ctx: FileContext) -> Vec<Json> {
    let opts = DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    };
    dump_input(name.as_bytes(), data.to_vec().into(), &opts)
        .lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The oracle's records of a file whose second object's resave differs
/// from its raw bytes (`first_diff` 4) are the dump's, line for line.
#[test]
fn a_resave_that_differs_is_recorded_as_the_oracle_records_it() {
    let (data, oracle) = oracle_input("own_tree");
    assert_eq!(oracle[2]["l3"]["resave"], "differs");
    assert_eq!(
        records_of("own_tree", &data, FileContext::unitypy()),
        oracle
    );
}

/// An object whose own embedded tree does not compile (deeper than
/// `max_tree_depth`) keeps the root type and the source UnityPy's
/// `_get_typetree_node()` gives it, the embedded node's: only its read's
/// outcome is the refusal, `other` with the limit's note (`PARITY.md`
/// section 19.3, "typetree depth"), so the rest of its record is the
/// oracle's, and so is every other record of the input.
#[test]
fn an_embedded_tree_that_does_not_compile_keeps_its_root_type_and_source() {
    let (data, oracle) = oracle_input("own_tree");
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_tree_depth = 3;
    let got = records_of("own_tree", &data, ctx);
    assert_eq!(got.len(), oracle.len());
    assert_eq!(got[0], oracle[0]);
    assert_eq!(got[2], oracle[2]);
    let (ours, theirs) = (&got[1], &oracle[1]);
    assert_eq!(ours["l1"], theirs["l1"]);
    assert_eq!(theirs["l2"]["source"], "embedded");
    assert_eq!(ours["l2"]["root_type"], theirs["l2"]["root_type"]);
    assert_eq!(ours["l2"]["source"], theirs["l2"]["source"]);
    let error = &ours["l2"]["error"];
    assert_eq!(
        (&error["kind"], &error["bytes_read"]),
        (&"other".into(), &Json::Null)
    );
    let note = s(&error["note"]);
    let refusal = note.strip_prefix("limit TreeDepth: ");
    let (value, max) = refusal
        .and_then(|r| r.split_once(" > "))
        .unwrap_or_else(|| panic!("{note}"));
    assert!(value.parse::<u64>().unwrap() > 3 && max == "3", "{note}");
    assert_eq!(ours["l2"]["value"], Json::Null);
    for k in ["raw_size", "raw_sha256"] {
        assert_eq!(ours["l3"][k], theirs["l3"][k]);
    }
    assert_eq!(ours["l3"]["resave"], "skipped");
}
