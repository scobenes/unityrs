//! The oracle's whole run on corpus indexes, replayed through the `unityrs`
//! binary: `tools/fixtures/index.py` ran `unitypy_dump.py --index` on each
//! index text (`unityrs-dump`'s `tests/fixtures/dump/index.json` "runs" and
//! "depth"), with argument bytes that are not always UTF-8, and recorded
//! its exit status and the names it dumped. `unityrs dump` exits as it
//! did and dumps the same names: an index Python's `json.loads` reads is
//! read (`NaN`, `1e400`, an integer of 4,300 digits, a lone surrogate, the
//! deepest nesting it decodes), one it refuses is refused with status 1,
//! and a `--kind` or `--build` that is not UTF-8 selects what it selects.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;
use unityrs_dump::json::hex;

fn fixture() -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../unityrs-dump/tests/fixtures/dump/index.json");
    let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&text).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A folder outside the repository holding the files the indexes name,
/// `f` and the byte 0xFF; removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new(what: &str) -> Self {
        let base = std::env::temp_dir().join(format!("unityrs-cli-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        for name in [&b"f"[..], b"\xff"] {
            std::fs::write(base.join(OsStr::from_bytes(name)), b"f").unwrap();
        }
        Self(base)
    }

    /// `unityrs dump --index` on `text` with `extra` arguments: the exit
    /// status, and the names of the file records it dumped when it wrote a
    /// dump.
    fn run(&self, text: &[u8], extra: &[Vec<u8>]) -> (i32, Option<Vec<String>>) {
        let index = self.0.join("index.json");
        std::fs::write(&index, text).unwrap();
        let out = self.0.join("out.jsonl");
        let o = Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("dump")
            .arg("--index")
            .arg(&index)
            .arg("-q")
            .arg("-o")
            .arg(&out)
            .args(extra.iter().map(|a| OsStr::from_bytes(a)))
            .output()
            .expect("the unityrs binary runs");
        let names = std::fs::read_to_string(&out).ok().map(|dump| {
            dump.lines()
                .map(|l| serde_json::from_str::<Json>(l).unwrap())
                .filter(|r| r["rec"] == "file")
                .map(|r| match &r["file"][0] {
                    Json::String(n) => hex(n.as_bytes()),
                    other => other["hex"].as_str().unwrap().to_owned(),
                })
                .collect()
        });
        let _ = std::fs::remove_file(&out);
        (o.status.code().unwrap(), names)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(v: &Json) -> Option<Vec<String>> {
    v.as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_owned()).collect())
}

/// Review rounds 6 and 7: the index was read with `serde_json`, so every
/// index here that Python reads but `serde_json` does not (`NaN`, `1e400`, a
/// lone surrogate, an integer past `u64`) failed the run with status 1 and
/// no dump, and a `--kind` or `--build` that was not UTF-8 was a usage
/// error (status 2).
#[test]
fn every_index_runs_as_the_oracles_run() {
    let f = fixture();
    let runs = f["runs"].as_array().unwrap();
    assert!(runs.len() >= 30, "{} runs", runs.len());
    let tmp = Tmp::new("index-runs");
    let mut failed = Vec::new();
    for c in runs {
        let extra: Vec<Vec<u8>> = c["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| unhex(a.as_str().unwrap()))
            .collect();
        let got = tmp.run(&unhex(c["index_hex"].as_str().unwrap()), &extra);
        let want = (
            i32::try_from(c["status"].as_i64().unwrap()).unwrap(),
            names(&c["names"]),
        );
        if got != want {
            failed.push(format!(
                "{}: got {got:?}, the oracle's {want:?}",
                c["name"].as_str().unwrap()
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The deepest nesting the oracle's run reads, and one level more, which
/// it refuses (`index.json` "depth"): arrays and objects.
#[test]
fn the_deepest_index_runs_as_the_oracles_run() {
    let f = fixture();
    let tmp = Tmp::new("index-depth");
    let mut failed = Vec::new();
    for c in f["depth"]["runs"].as_array().unwrap() {
        let levels = usize::try_from(c["levels"].as_u64().unwrap()).unwrap();
        let n = levels - 1;
        let inner = if c["kind"] == "array" {
            "[".repeat(n) + &"]".repeat(n)
        } else {
            r#"{"a":"#.repeat(n) + "0" + &"}".repeat(n)
        };
        let text = format!(r#"{{"files":[{{"path":"f","kind":"bundle"}}],"x":{inner}}}"#);
        let got = tmp.run(text.as_bytes(), &[]);
        let want = (
            i32::try_from(c["status"].as_i64().unwrap()).unwrap(),
            names(&c["names"]),
        );
        if got != want {
            failed.push(format!(
                "{} {levels}: got {got:?}, want {want:?}",
                c["kind"]
            ));
        }
    }
    assert_eq!(failed.len(), 0, "{}", failed.join("\n"));
}
