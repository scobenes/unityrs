//! Replays `tools/fixtures/dump.py`'s oracle fixtures without Python (plan
//! section 6, G4): `canonical.py`'s encodings of synthetic values and names,
//! Python's decimal form of packed integers, `unitypy_dump.py`'s input
//! selection and `fnmatch.fnmatchcase`, and `dump_records.py`'s `error_kind` on
//! `check_read` messages.

mod common;

use common::{Tmp, fixture, parsed, s, unhex};
use serde_json::Value as Json;
use unityrs_core::walk::L2Kind;
use unityrs_dump::encode::encode;
use unityrs_dump::input::check_read_kind;
use unityrs_dump::json::{limbs_to_decimal, write_name};
use unityrs_dump::select::{Selection, collect, fnmatchcase};

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
        let strings = |k: &str| -> Option<Vec<String>> {
            args.get(k).map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| s(x).to_owned())
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
                    i.sha256,
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
