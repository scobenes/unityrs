//! `JOB.json` itself (`docs/BATCH_SPEC.md` section 1, section 10's first
//! rules; `docs/plans/extract.md` X8's spec rows that phase 1 reaches): a
//! missing, unreadable, FIFO or over-64 MiB file, one behind a symlink, a
//! `deps` list that repeats an id or passes `max_view_bundles`, and a view
//! one past `max_view_bundles`.

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::base;
use serde_json::json;
use unityrs_batch::spec::{self, MAX_SPEC_BYTES};
use unityrs_testkit::scratch::Scratch;
use unityrs_testkit::{child, guard};

fn load(path: &Path) -> String {
    match spec::load(path) {
        Ok(_) => "ok".to_owned(),
        Err(e) => e.detail(),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write the spec");
}

#[test]
fn a_valid_file_loads() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    write(&path, base().to_string().as_bytes());
    assert_eq!(load(&path), "ok");
}

#[test]
fn a_missing_file_is_unreadable() {
    let dir = Scratch::new("batch-job");
    assert_eq!(load(&dir.join("JOB.json")), "unreadable");
    // A path through a file is no path.
    let file = dir.join("f");
    write(&file, b"{}");
    assert_eq!(load(&file.join("JOB.json")), "unreadable");
}

#[test]
fn a_file_that_cannot_be_opened_is_unreadable() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    write(&path, base().to_string().as_bytes());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
    if File::open(&path).is_ok() {
        // A privileged user (root, or `CAP_DAC_OVERRIDE`) opens it anyway:
        // there is nothing to refuse, and the spec is valid.
        assert_eq!(load(&path), "ok");
    } else {
        assert_eq!(load(&path), "unreadable");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod back");
}

/// [`load`] on a thread of its own, failing the test rather than hanging
/// it when the load blocks (past the testkit's deadlock guard: a load that
/// does not block answers at once).
fn load_within(path: &Path) -> String {
    let path = path.to_path_buf();
    guard::finishes("a load that must not block on a FIFO", move || load(&path))
}

fn mkfifo(path: &Path) {
    let status = child::status(Command::new("mkfifo").arg(path)).expect("run mkfifo");
    assert!(status.success(), "mkfifo: {status}");
}

#[test]
fn a_fifo_is_not_regular_and_does_not_block() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    mkfifo(&path);
    // No writer holds the FIFO: a blocking open would wait for ever. The
    // FIFO stays unopened by any writer, so a hang here is the test's
    // thread, left behind, and never the process's end.
    assert_eq!(load_within(&path), "not_regular");
}

#[test]
fn a_symlink_is_followed() {
    let dir = Scratch::new("batch-job");
    let file = dir.join("spec.json");
    write(&file, base().to_string().as_bytes());
    let link = |name: &str, to: &Path| -> PathBuf {
        let l = dir.join(name);
        symlink(to, &l).expect("symlink");
        l
    };
    assert_eq!(load(&link("to-file", &file)), "ok");
    let fifo = dir.join("fifo");
    mkfifo(&fifo);
    assert_eq!(load_within(&link("to-fifo", &fifo)), "not_regular");
    assert_eq!(load(&link("dangling", &dir.join("none"))), "unreadable");
    let a = dir.join("loop-a");
    let b = dir.join("loop-b");
    symlink(&b, &a).expect("symlink");
    symlink(&a, &b).expect("symlink");
    assert_eq!(load(&a), "unreadable");
}

#[test]
fn a_directory_is_not_regular() {
    let dir = Scratch::new("batch-job");
    assert_eq!(load(dir.path()), "not_regular");
}

#[test]
fn a_file_over_64_mib_is_too_large() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    // A valid spec padded with spaces to exactly 64 MiB is taken.
    let text = base().to_string();
    let mut f = File::create(&path).expect("create");
    f.write_all(text.as_bytes()).expect("write");
    let pad = usize::try_from(MAX_SPEC_BYTES).unwrap() - text.len();
    f.write_all(&vec![b' '; pad]).expect("pad");
    drop(f);
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_SPEC_BYTES);
    assert_eq!(load(&path), "ok");
    // One byte more is refused, before the parse.
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(b" "))
        .expect("append");
    assert_eq!(load(&path), "too_large");
    // A sparse one far past the cap is refused by its size alone.
    let sparse = dir.join("sparse.json");
    File::create(&sparse)
        .and_then(|f| f.set_len(1 << 40))
        .expect("sparse file");
    assert_eq!(load(&sparse), "too_large");
}

#[test]
fn bytes_that_are_not_utf8_then_syntax() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    write(&path, b"{\"run_id\": \"\xff\"}");
    assert_eq!(load(&path), "not_utf8");
    write(&path, b"{\"spec_version\": 1,");
    assert_eq!(load(&path), "syntax");
    write(&path, b"");
    assert_eq!(load(&path), "syntax");
    // Past serde_json's 128 levels.
    write(
        &path,
        format!("{}{}", "[".repeat(129), "]".repeat(129)).as_bytes(),
    );
    assert_eq!(load(&path), "syntax");
    // A lone surrogate escape is no string.
    write(&path, br#"{"run_id": "\ud800"}"#);
    assert_eq!(load(&path), "syntax");
}

#[test]
fn a_deps_list_that_repeats_an_id() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    let mut v = base();
    v["inputs"][0]["deps"] = json!(["d", "a", "d"]);
    write(&path, v.to_string().as_bytes());
    assert_eq!(load(&path), "deps /inputs/0/deps");
}

#[test]
fn a_deps_list_past_max_view_bundles() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    let mut v = base();
    v["dependencies"] = json!([
        {"id": "d", "path": "/d"}, {"id": "e", "path": "/e"}, {"id": "f", "path": "/f"},
    ]);
    v["inputs"][0]["deps"] = json!(["d", "e", "f"]);
    v["max_view_bundles"] = json!(4);
    write(&path, v.to_string().as_bytes());
    assert_eq!(load(&path), "ok");
    // The list alone passes the bound, before any view is walked.
    v["max_view_bundles"] = json!(2);
    write(&path, v.to_string().as_bytes());
    assert_eq!(load(&path), "deps /inputs/0/deps");
}

#[test]
fn a_view_one_past_max_view_bundles() {
    let dir = Scratch::new("batch-job");
    let path = dir.join("JOB.json");
    // A chain a -> d0 -> d1 -> ... -> d(n-1): `a`'s view holds n + 1.
    let n = 5;
    let mut v = base();
    let deps: Vec<_> = (0..n)
        .map(|i| {
            let next: Vec<String> = if i + 1 < n {
                vec![format!("d{}", i + 1)]
            } else {
                vec![]
            };
            json!({"id": format!("d{i}"), "path": format!("/d{i}"), "deps": next})
        })
        .collect();
    v["dependencies"] = json!(deps);
    v["inputs"][0]["deps"] = json!(["d0"]);
    v["max_view_bundles"] = json!(n + 1);
    write(&path, v.to_string().as_bytes());
    assert_eq!(load(&path), "ok");
    v["max_view_bundles"] = json!(n);
    write(&path, v.to_string().as_bytes());
    assert_eq!(load(&path), "view_size /inputs/0");
}
