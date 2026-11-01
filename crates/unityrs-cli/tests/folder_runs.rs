//! The oracle's whole run on deep input folders, replayed through
//! `unityrs-dump`'s selection and through the `unityrs` binary:
//! `tools/fixtures/folders.py` ran `unitypy_dump.py FOLDER` on chains of
//! folders (`unityrs-dump`'s `tests/fixtures/dump/folders.json`) and recorded
//! its exit status and the names it dumped. Its `os.walk` lists folders
//! [`MAX_FOLDER_DEPTH`] deep and fails the run one level deeper
//! (`RecursionError`: status 1, no dump), whatever the deepest level holds;
//! and it holds one open folder at a time, so a low limit on open files
//! (set in the child, as the oracle's was) leaves no folder unread.
//!
//! Review round 8: `select::walk` had no depth bound, so unityrs dumped what
//! the oracle refused, and it held every ancestor's folder open, so under a
//! limit of 64 open files it silently left out the files of a chain 100
//! folders deep, with status 0.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value as Json;
use unityrs_dump::select::{MAX_FOLDER_DEPTH, Selection, collect};
use unityrs_testkit::child;

fn fixture() -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../unityrs-dump/tests/fixtures/dump/folders.json");
    let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&text).unwrap()
}

/// A folder outside the repository holding one tree of the fixture's
/// (`folders.py`'s module docs): the input folder `in` with the file `top`
/// and a chain of `levels` folders named `d`, the deepest level by `shape`.
/// Removed when dropped.
struct Tree {
    base: PathBuf,
    top: PathBuf,
}

impl Tree {
    fn new(shape: &str, levels: usize) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "unityrs-cli-folders-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        remove(&base);
        std::fs::create_dir_all(&base).unwrap();
        let top = std::fs::canonicalize(&base).unwrap().join("in");
        std::fs::create_dir(&top).unwrap();
        std::fs::write(top.join("top"), b"t").unwrap();
        let mut deepest = top.clone();
        for _ in 0..if shape == "link" { levels - 1 } else { levels } {
            deepest.push("d");
            std::fs::create_dir(&deepest).unwrap();
        }
        match shape {
            "file" => std::fs::write(deepest.join("f"), b"f").unwrap(),
            "empty" => {}
            "link" => std::os::unix::fs::symlink(&top, deepest.join("d")).unwrap(),
            "unreadable" => {
                std::fs::set_permissions(&deepest, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            other => panic!("no shape {other}"),
        }
        Self { base, top }
    }

    /// `unityrs dump` on the input folder, run through `sh` with its limit
    /// on open files set to `nofile` when it is given: the exit status, and
    /// the names of the file records it dumped when it wrote a dump.
    fn run(&self, nofile: Option<u64>) -> (i32, Option<Vec<String>>) {
        let out = self.base.join("out.jsonl");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(match nofile {
            Some(n) => format!("ulimit -n {n} && exec \"$@\""),
            None => "exec \"$@\"".to_owned(),
        });
        cmd.arg("sh")
            .arg(env!("CARGO_BIN_EXE_unityrs"))
            .arg("dump")
            .arg(&self.top)
            .arg("-q")
            .arg("-o")
            .arg(&out);
        let o = child::output(&mut cmd).expect("sh runs the unityrs binary");
        let names = std::fs::read_to_string(&out).ok().map(|dump| {
            dump.lines()
                .map(|l| serde_json::from_str::<Json>(l).unwrap())
                .filter(|r| r["rec"] == "file")
                .map(|r| r["file"][0].as_str().unwrap().to_owned())
                .collect()
        });
        let _ = std::fs::remove_file(&out);
        let status = o.status.code().unwrap_or_else(|| panic!("{o:?}"));
        (status, names)
    }

    /// `select::collect` on the input folder: its names, or its error.
    fn collect(&self) -> Result<Vec<String>, String> {
        let sel = Selection {
            inputs: vec![self.top.clone()],
            ..Selection::default()
        };
        collect(&sel).map(|items| {
            items
                .into_iter()
                .map(|i| String::from_utf8(i.name).unwrap())
                .collect()
        })
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        remove(&self.base);
    }
}

/// Removes `base` and everything in it a folder at a time, so a chain past
/// the stack or the open-file limit, and a folder of mode 000, go too.
fn remove(base: &Path) {
    let mut pending = vec![base.to_path_buf()];
    let mut folders = Vec::new();
    while let Some(dir) = pending.pop() {
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(e.path());
            } else {
                let _ = std::fs::remove_file(e.path());
            }
        }
        folders.push(dir);
    }
    for dir in folders.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
}

fn want(c: &Json) -> (i32, Option<Vec<String>>) {
    let names = c["names"]
        .as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_owned()).collect());
    (i32::try_from(c["status"].as_i64().unwrap()).unwrap(), names)
}

fn usize_of(v: &Json) -> usize {
    usize::try_from(v.as_u64().unwrap()).unwrap()
}

/// The deepest folder the oracle lists, and one level deeper, for each
/// shape of the deepest level (`folders.json` "depth"): the selection and
/// the binary each walk the first and fail the run on the second, as the
/// oracle does.
#[test]
fn the_deepest_folder_the_oracle_walks_is_walked_and_no_deeper() {
    let f = fixture();
    assert_eq!(MAX_FOLDER_DEPTH, usize_of(&f["depth"]["max"]));
    let runs = f["depth"]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 8, "four shapes, at the deepest and one past it");
    let mut failed = Vec::new();
    for c in runs {
        let (shape, levels) = (c["shape"].as_str().unwrap(), usize_of(&c["levels"]));
        let tree = Tree::new(shape, levels);
        let (status, names) = want(c);
        let got = tree.run(None);
        if got != (status, names.clone()) {
            failed.push(format!(
                "{shape} {levels}: the binary {got:?}, the oracle's {:?}",
                (status, &names)
            ));
        }
        let selected = tree.collect();
        let ok = match (&selected, &names) {
            (Ok(got), Some(names)) => got == names,
            (Err(e), None) => status == 1 && e.contains("maximum recursion depth"),
            _ => false,
        };
        if !ok {
            failed.push(format!(
                "{shape} {levels}: collect {selected:?}, the oracle's {:?}",
                (status, &names)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// A chain of folders walked under a low limit on open files
/// (`folders.json` "nofile"): the binary dumps every file the oracle
/// dumped under the same limit.
#[test]
fn a_deep_walk_under_a_low_limit_on_open_files_reads_every_folder() {
    let f = fixture();
    let c = &f["nofile"];
    let tree = Tree::new(c["shape"].as_str().unwrap(), usize_of(&c["levels"]));
    let limit = c["limit"].as_u64().unwrap();
    let want = want(c);
    assert!(
        want.1.as_ref().is_some_and(|n| n.len() == 2),
        "the oracle dumps the deepest file: {want:?}"
    );
    assert_eq!(tree.run(Some(limit)), want, "under {limit} open files");
}
