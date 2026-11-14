//! What `batch.rs` and `batch_write.rs` share: the binary, a spec, the
//! exact `exit` line of a 78, and the pipes they fill.

// Each test file uses its own part.
#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use unityrs_batch::progress::{Exit, Snapshot};
use unityrs_batch::reason::Code;
use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;

pub const BIN: &str = env!("CARGO_BIN_EXE_unityrs");

/// `nobody`'s uid and gid, which a test run as root runs its children
/// under ([`unprivileged`]).
const NOBODY: u32 = 65_534;

/// Whether the tests run as root.
pub fn root() -> bool {
    // SAFETY: `getuid` takes no arguments, touches no memory and always
    // succeeds.
    unsafe { libc::getuid() == 0 }
}

/// `cmd`, under `nobody` when the tests run as root, so a permission
/// (a `JOB.json` at mode 0000, a pipe `fchmod`ed to 0000) binds it as it
/// binds a caller: root's `CAP_DAC_OVERRIDE` would read through both.
pub fn unprivileged(cmd: &mut Command) -> &mut Command {
    if root() {
        cmd.uid(NOBODY).gid(NOBODY)
    } else {
        cmd
    }
}

/// `dir`, made enterable by [`unprivileged`]'s children when the tests run
/// as root (its own mode 0711; the scratch root above it is the testkit's).
pub fn enterable(dir: &Scratch) {
    if root() {
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711)).unwrap();
    }
}

pub fn unityrs(args: &[&str]) -> Output {
    child::output(unprivileged(Command::new(BIN).args(args))).expect("the unityrs binary runs")
}

/// A spec with two inputs, its other members `extra`'s.
pub fn job(extra: &str) -> String {
    format!(
        r#"{{"spec_version":1,"run_id":"r","attempt":1,"threads":2,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":268435456,"max_decoded_bytes":16777216,
        "dep_cache_bytes":33554432,"max_view_bundles":4,
        "spool":{{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0}},
        "outputs":{{"textures":{{"key":"{{input}}/{{name}}.png"}}}},
        "sink":{{"kind":"directory","root":"/out"}}{extra}}}"#
    )
}

pub const INPUTS: &str = r#","inputs":[{"id":"a","path":"/a"},{"id":"b","path":"/b"}]"#;

pub fn write_job(dir: &Scratch, text: &str) -> PathBuf {
    let p = dir.join("JOB.json");
    std::fs::write(&p, text).unwrap();
    p
}

pub fn line_of(o: &Output) -> (String, Value) {
    let text = String::from_utf8(o.stdout.clone()).unwrap();
    assert!(text.ends_with('\n'), "{text:?}");
    assert_eq!(text.matches('\n').count(), 1, "one line: {text:?}");
    let v = serde_json::from_str(&text).unwrap();
    (text, v)
}

pub fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

/// The `exit` line of a 78 at `ms` with `rss`.
pub fn refusal_line(ms: u64, rss: u64, detail: &str) -> String {
    let mut s = Snapshot::validate(ms);
    s.memory.rss = rss;
    s.line(Some(&Exit {
        status: None,
        code: Code::BadSpec,
        reason: Some("bad_spec"),
        detail: Some(detail.into()),
    }))
}

/// A 78 of `unityrs batch`: its exact `exit` line, and its stderr line
/// first.
pub fn assert_refused(o: &Output, detail: &str) {
    assert_refused_by(o, "unityrs batch", detail);
}

/// A 78 whose stderr line is `<prog>: bad_spec <detail>`.
pub fn assert_refused_by(o: &Output, prog: &str, detail: &str) {
    assert_eq!(o.status.code(), Some(78), "{o:?}");
    let (text, v) = line_of(o);
    let want = refusal_line(u(&v["ms"]), u(&v["memory"]["rss"]), detail);
    assert_eq!(text, want);
    let err = String::from_utf8_lossy(&o.stderr);
    let first = format!("{prog}: bad_spec {detail}\n");
    assert!(err.starts_with(&first), "{err:?}");
}

pub fn check(path: &Path) -> Output {
    unityrs(&["batch", "--check", path.to_str().unwrap()])
}

/// A run of `unityrs batch --check <path>` with `stdout` and `stderr`; its
/// exit status and how long it took. The `Command`, which holds both, is
/// dropped before this returns.
pub fn check_to(path: &Path, stdout: Stdio, stderr: Stdio) -> (Option<i32>, Duration) {
    let start = Instant::now();
    let status = child::status(unprivileged(
        Command::new(BIN)
            .args(["batch", "--check", path.to_str().unwrap()])
            .stdout(stdout)
            .stderr(stderr),
    ))
    .unwrap();
    (status.code(), start.elapsed())
}

/// Fills the pipe (or socket) behind `fd` through a non-blocking
/// description of its own, as another writer would, and keeps it. Blocks,
/// then single bytes, so no room is left.
pub fn fill(fd: &impl AsRawFd) -> File {
    let w = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .unwrap();
    fill_file(&w);
    w
}

/// Writes `w`, non-blocking, until it is full.
pub fn fill_file(mut w: &File) {
    for block in [&[b'x'; 4096][..], b"x"] {
        loop {
            match w.write(block) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
    }
}

pub fn read_all(mut r: impl Read) -> Vec<u8> {
    let mut got = Vec::new();
    r.read_to_end(&mut got).unwrap();
    got
}
