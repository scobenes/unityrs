//! Replays `tools/fixtures/index.py`'s oracle fixture (`index.json`)
//! without Python: Python's `str()` of every kind of value an index holds,
//! which `--build` compares, and `collect_inputs`' own selections from an
//! index by `--kind` and `--build`, with each pin as the run's message
//! prints it, and the deepest index `json.loads` decodes in the oracle's
//! run. The oracle's whole runs on index texts are the CLI's
//! `tests/index_runs.rs`.

mod common;

use common::{Tmp, fixture, s, unhex};
use serde_json::Value as Json;
use unityrs_dump::json::hex;
use unityrs_dump::select::index::{MAX_DEPTH, parse};
use unityrs_dump::select::{PyStr, Selection, collect, py_str};

/// Review round 7: `--build` compared JSON text for a list or an object,
/// `serde_json`'s misrounded float for a float (`968494.1733294675` read as
/// `968494.1733294677`) and `-0.0` for the integer `-0`, so it selected
/// other inputs than the oracle, and an index holding `NaN`, `1e400`, a
/// lone surrogate or an integer past `u64` was refused. Every value reads
/// to the oracle's `str()`.
#[test]
fn every_values_str_is_pythons() {
    let f = fixture("index.json");
    let cases = f["str"].as_array().unwrap();
    assert!(cases.len() > 500, "{} cases", cases.len());
    let mut failed = Vec::new();
    for c in cases {
        let text = s(&c["json"]);
        match parse(text.as_bytes()) {
            Ok(v) => {
                let got = hex(py_str(&v).as_bytes());
                if got != s(&c["str"]) {
                    failed.push(format!(
                        "{text}: {:?}, Python's {:?}",
                        String::from_utf8_lossy(&unhex(&got)),
                        String::from_utf8_lossy(&unhex(s(&c["str"])))
                    ));
                }
            }
            Err(e) => failed.push(format!("{text}: refused ({e})")),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The comma list an argument names, as `main` splits it.
fn items(arg: &Json) -> Option<Vec<PyStr>> {
    let bytes = unhex(arg.as_str()?);
    Some(
        bytes
            .split(|b| *b == b',')
            .map(PyStr::from_os_bytes)
            .collect(),
    )
}

/// `collect_inputs` itself (`index.json` "select") over an index of entries
/// whose `build` is a value of every kind, and `--build` naming each one's
/// `str()` or a near miss; whose `kind` is a lone surrogate, a number, a
/// list or another case, against a `--kind` that is not UTF-8; whose `path`
/// is a lone surrogate (a byte), any other surrogate (the run fails), or
/// is repeated; and whose pins print as the oracle's message prints them.
#[test]
fn inputs_are_selected_as_collect_inputs_selects_them() {
    let f = fixture("index.json");
    let cases = f["select"].as_array().unwrap();
    let tmp = Tmp::new("index-select");
    let mut failed = Vec::new();
    for (n, c) in cases.iter().enumerate() {
        let index = tmp.0.join(format!("{n}.json"));
        std::fs::write(&index, s(&c["index"])).unwrap();
        let mut sel = Selection {
            inputs: vec![index],
            index: true,
            builds: items(&c["build"]),
            ..Selection::default()
        };
        if let Some(kinds) = items(&c["kind"]) {
            sel.kinds = kinds;
        }
        let got: Option<Vec<(String, Option<String>)>> = collect(&sel).ok().map(|inputs| {
            inputs
                .iter()
                .map(|i| {
                    let pin = i.sha256.as_ref().map(|p| hex(p.to_string().as_bytes()));
                    (hex(&i.name), pin)
                })
                .collect()
        });
        let want: Option<Vec<(String, Option<String>)>> = c["inputs"].as_array().map(|rows| {
            rows.iter()
                .map(|r| (s(&r[0]).to_owned(), r[1].as_str().map(str::to_owned)))
                .collect()
        });
        if got != want {
            failed.push(format!(
                "{}:\n  got  {got:?}\n  want {want:?}",
                s(&c["name"])
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// An index whose member nests `levels` arrays or objects in all.
fn nested(levels: usize, kind: &str) -> String {
    let n = levels - 1;
    let inner = if kind == "array" {
        "[".repeat(n) + &"]".repeat(n)
    } else {
        r#"{"a":"#.repeat(n) + "0" + &"}".repeat(n)
    };
    format!(r#"{{"files":[{{"path":"f","kind":"bundle"}}],"x":{inner}}}"#)
}

/// The deepest nesting the oracle's `json.loads` decodes in
/// `collect_inputs` (`index.json` "depth", found by running it) is
/// [`MAX_DEPTH`]: an index that deep is read, one level deeper is refused,
/// as Python's `RecursionError` refuses it. Review round 6: `serde_json`
/// refused past 128.
#[test]
fn the_deepest_index_the_oracle_decodes_is_read_and_no_deeper() {
    let f = fixture("index.json");
    let max = usize::try_from(f["depth"]["max"].as_u64().unwrap()).unwrap();
    assert_eq!(MAX_DEPTH, max);
    for kind in ["array", "object"] {
        assert!(parse(nested(max, kind).as_bytes()).is_ok(), "{kind}");
        let e = parse(nested(max + 1, kind).as_bytes()).unwrap_err();
        assert!(e.contains("maximum recursion depth"), "{kind}: {e}");
    }
}
