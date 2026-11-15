//! What the process host's scenes share (`batch_lifetime.rs`,
//! `batch_host.rs`): a run's spec, its start through the scene, its end,
//! and the trace and lifecycle points it is held and watched at.

// Each test file uses its own part.
#![allow(dead_code)]

use super::common;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use unityrs_testkit::child::{self, Fifo, Live, Scene};

/// A run's spec in `dir`: two inputs, small files that are not bundles.
pub fn job(dir: &Path, edit: impl FnOnce(&mut Value)) -> PathBuf {
    for name in ["spool", "sink"] {
        fs::create_dir_all(dir.join(name)).unwrap();
    }
    for name in ["a.bin", "b.bin"] {
        fs::write(dir.join(name), b"not a bundle").unwrap();
    }
    let mut spec: Value = serde_json::from_str(&common::job(
        r#","inputs":[{"id":"a","path":"/a"},{"id":"b","path":"/b"}]"#,
    ))
    .unwrap();
    spec["spool"]["dir"] = json!(dir.join("spool"));
    spec["sink"]["root"] = json!(dir.join("sink"));
    spec["inputs"][0]["path"] = json!(dir.join("a.bin"));
    spec["inputs"][1]["path"] = json!(dir.join("b.bin"));
    edit(&mut spec);
    let path = dir.join("job.json");
    fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
    path
}
/// RFC 3339 for `ahead` from now, whole seconds.
pub fn deadline(ahead: Duration) -> String {
    let at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + ahead;
    let text = unityrs_batch::sys::utc(i64::try_from(at.as_secs()).unwrap()).unwrap();
    String::from_utf8(text.to_vec()).unwrap()
}
pub struct Started {
    pub live: Live,
    pub out: (
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::thread::JoinHandle<()>,
    ),
    pub err: (
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::thread::JoinHandle<()>,
    ),
}
pub fn start(scene: &mut Scene, job: &Path, env: &[(&str, &str)], stdin: Stdio) -> Started {
    start_with(scene, job, env, stdin, |_| {})
}
/// [`start`], `edit` amending the command first (its `pre_exec`).
pub fn start_with(
    scene: &mut Scene,
    job: &Path,
    env: &[(&str, &str)],
    stdin: Stdio,
    edit: impl FnOnce(&mut Command),
) -> Started {
    let mut cmd = Command::new(common::BIN);
    cmd.arg("batch")
        .arg(job)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    edit(&mut cmd);
    let live = child::live(scene, &mut cmd, false).unwrap();
    let out = child::drain(scene.take_stdout(live).unwrap());
    let err = child::drain(scene.take_stderr(live).unwrap());
    Started { live, out, err }
}
/// The process's status, its last stdout line (the `exit` line) and stderr.
pub fn ended(scene: &mut Scene, s: Started) -> (i32, Value, String) {
    let status = scene.wait(s.live).unwrap();
    let collect = |(rx, thread): (
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::thread::JoinHandle<()>,
    )| {
        thread.join().unwrap();
        rx.try_iter().flatten().collect::<Vec<u8>>()
    };
    let out = String::from_utf8(collect(s.out)).unwrap();
    let err = String::from_utf8(collect(s.err)).unwrap();
    let last = out
        .lines()
        .last()
        .unwrap_or_else(|| panic!("no exit line: {err}"));
    let line = serde_json::from_str(last).unwrap();
    (status.code().unwrap_or(-1), line, err)
}
/// Wait until the `trace` log holds `line`, the child still running.
pub fn traced(scene: &mut Scene, live: Live, trace: &Path, want: &str) {
    let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
    while !fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .any(|l| l == want)
    {
        assert!(scene.running(live).unwrap(), "ended before {want:?}");
        assert!(Instant::now() < until, "no {want:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}
/// Wait for a lifecycle trace point on its FIFO.
pub fn lifecycle(scene: &mut Scene, live: Live, fifo: &Fifo, point: &str) {
    while fifo.line_while(scene, live).unwrap() != point {}
}
pub fn head(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join("sink/_unityrs/r/manifest.json")).unwrap()).unwrap()
}
/// The scene's paths: `trace`, `go` (the hold's release) and the env that
/// holds the controller at its wait. A `trace` needs its lifecycle FIFO
/// read (`trace.lifecycle`, a [`Fifo`]), or step 1 refuses it.
pub fn held(dir: &Path) -> (PathBuf, PathBuf, String) {
    let (trace, go) = (dir.join("trace"), dir.join("go"));
    let point = format!("poll=wait:{}", go.display());
    (trace, go, point)
}
