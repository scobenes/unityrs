//! Child processes a test runs to their end, and the one rule every such
//! test keeps: a child the system kills from outside is no result.
//!
//! A child killed by `SIGKILL` did not end itself: nothing a program does
//! raises `SIGKILL` but a `kill` of it, and a test that runs a child to its
//! end sends none, so the OOM killer (or a person) ended it from outside,
//! whatever the child would have come to. Its run says nothing about the
//! code under test, but the child is the same on the next run, so
//! [`output`] and [`status`] run it again, up to [`KILLED_RUNS`] runs in
//! all, and a child killed on every one of them fails the test, saying the
//! system killed it. Every other ending is the child's own and comes back
//! at once, after one run: an exit status, an abort (`SIGABRT`: the code
//! under test's own abort, or Rust's report of a stack overflow), any other
//! signal. Live scenes use [`scene`] and [`live`]; their explicit
//! `Scene::kill` marks a deliberate kill, which is never retried. All other
//! outside kills restart the entire closure, including its services and FIFOs.
//!
//! Every Rust test in the workspace that runs a child process runs it
//! through here: [`crate::stack`]'s and [`crate::address_space`]'s probes,
//! the counting allocator's tally-panic probe, the `unityrs` binary's
//! tests, the tests that run Python, `cargo`, `rustc`, `git` and `sh`.
//! (Review round 7 gave the rule to [`crate::stack::run`] alone; review
//! round 8 found the rest failing on one kill under memory pressure.) A
//! source scan keeps it so ([`direct_runs`]). The Python suites and tools
//! (`tools/`, `scripts/`) keep the same rule through `tools/parity/child.py`
//! (review round 9), which its own scan keeps.
//!
//! Each run of a child also gets a scratch folder this process owns and
//! removes when the run ends, however it ends, named in the child's
//! [`crate::scratch::VAR`], so what the child writes there is gone even
//! when it aborts or is killed ([`crate::scratch`]). A caller that sets the
//! variable on its command (`Command::env`) or removes it
//! (`Command::env_remove`) keeps what it chose.
//!
//! Owner: package B.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

#[cfg(unix)]
mod live;
#[cfg(unix)]
pub use live::{Fifo, Live, Scene, drain, live, scene};

/// The most runs [`output`] and [`status`] make of a child that is killed
/// by `SIGKILL` (module docs) before they fail the test.
pub const KILLED_RUNS: u32 = 4;

const KILL: i32 = 9;

/// Whether a child that ended with `status` was killed by `SIGKILL`.
#[must_use]
pub fn killed(status: ExitStatus) -> bool {
    signal(status) == Some(KILL)
}

#[cfg(unix)]
fn signal(status: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&status)
}

#[cfg(not(unix))]
fn signal(_: ExitStatus) -> Option<i32> {
    None
}

/// `cmd.output()`, run again while a `SIGKILL` from outside ends it, up to
/// [`KILLED_RUNS`] runs in all (module docs). Every other outcome is the
/// first run's, returned as `Command::output` returns it.
///
/// # Errors
/// When the child cannot start, as `Command::output`.
///
/// # Panics
/// When `SIGKILL` ends the child on every run: the message says the system
/// killed it, and ends with the last run's last lines of output.
#[track_caller]
pub fn output(cmd: &mut Command) -> io::Result<Output> {
    again(cmd, Command::output, |o| {
        (o.status, tail(&o.stdout), tail(&o.stderr))
    })
}

/// `cmd.status()` (the child's streams are this process's), run again
/// while a `SIGKILL` from outside ends it, as [`output`].
///
/// # Errors
/// When the child cannot start, as `Command::status`.
///
/// # Panics
/// When `SIGKILL` ends the child on every run, saying the system killed it.
#[track_caller]
pub fn status(cmd: &mut Command) -> io::Result<ExitStatus> {
    again(cmd, Command::status, |s| (*s, String::new(), String::new()))
}

/// Runs `cmd` by `run` until it ends other than by `SIGKILL`, at most
/// [`KILLED_RUNS`] times; `ending` gives a run's status and the last lines
/// of its output, for the message.
#[track_caller]
fn again<T>(
    cmd: &mut Command,
    run: impl Fn(&mut Command) -> io::Result<T>,
    ending: impl Fn(&T) -> (ExitStatus, String, String),
) -> io::Result<T> {
    let mut runs = 0;
    // A caller that set or removed the variable keeps what it chose.
    let chosen = cmd.get_envs().any(|(k, _)| k == crate::scratch::VAR);
    loop {
        // The run's own folder, removed when this iteration ends, however
        // the child ended (crate::scratch).
        let own = (!chosen).then(|| crate::scratch::Scratch::new("child"));
        if let Some(own) = &own {
            cmd.env(crate::scratch::VAR, own.path());
        }
        let done = run(cmd)?;
        drop(own);
        runs += 1;
        let (status, out, err) = ending(&done);
        if !killed(status) {
            return Ok(done);
        }
        assert!(
            runs < KILLED_RUNS,
            "{cmd:?}: killed by SIGKILL on each of its {runs} runs: the system ended it \
             from outside (the OOM killer, or a person), not the code under test, so it \
             came to no result\n--- stdout\n{out}\n--- stderr\n{err}"
        );
    }
}

/// The last lines of a child's output, enough to say what it did.
pub(crate) fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// What runs a child process other than through this module: `Command`'s
/// own runs, by method (`.output()`, `.spawn()`) or by path.
pub const DIRECT: &[&str] = &[
    ".kill()",
    "Child::kill",
    ".output()",
    ".spawn()",
    "Command::output",
    "Command::status",
    "Command::spawn",
];

/// Every place under `root` (its `.rs` files, `target` directories left
/// out, this file too) that runs a child process other than through
/// [`output`] or [`status`]: each line outside a `//` comment that names
/// one of [`DIRECT`] (a thread's `spawn` takes its closure, so `.spawn()`
/// is a process's), and each file that makes more commands
/// (`Command::new(`) than it runs through this module (`child::output(`,
/// `child::status(`), which is how a `.status()` on a command shows (a
/// dump error's `.status()` is its exit code). Live children use `child::live`;
/// direct `Child::kill` is refused too, preserving deliberate-kill bookkeeping.
///
/// # Panics
/// When a directory or a file under `root` cannot be read.
#[must_use]
pub fn direct_runs(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    crate::guard::rust_files(root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        if path.ends_with("unityrs-testkit/src/child.rs")
            || path
                .parent()
                .is_some_and(|p| p.ends_with("unityrs-testkit/src/child"))
        {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (mut made, mut run) = (0, 0);
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if DIRECT.iter().any(|d| code.contains(d)) {
                found.push(format!("{}:{}: {}", path.display(), i + 1, code));
            }
            made += code.matches("Command::new(").count();
            run += code.matches("child::output(").count()
                + code.matches("child::status(").count()
                + code.matches("child::live(").count();
        }
        if made > run {
            found.push(format!(
                "{}: {made} commands made, {run} run through unityrs_testkit::child",
                path.display()
            ));
        }
    }
    found
}

/// A fresh directory of marks, one a run of a probe, removed when dropped
/// (the tests of the probes a `SIGKILL` from outside ends): a
/// [`crate::scratch::Scratch`] and its path.
#[cfg(test)]
pub(crate) struct Marks(
    pub(crate) std::path::PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")]
    crate::scratch::Scratch,
);

#[cfg(test)]
impl Marks {
    pub(crate) fn new(name: &str) -> Self {
        let dir = crate::scratch::Scratch::new(&format!("marks-{name}"));
        Self(dir.path().to_path_buf(), dir)
    }

    pub(crate) fn runs(&self) -> usize {
        std::fs::read_dir(&self.0).unwrap().count()
    }

    /// The probe spec that has a probe killed on each of its first `n`
    /// runs ([`kill_self_while`]).
    pub(crate) fn killed(&self, n: u32) -> String {
        format!("{n}:{}", self.0.display())
    }
}

/// In a probe, `N:DIR`: the run that finds fewer than `N` marks in `DIR`
/// adds one and is killed by `SIGKILL` ([`kill_self`]), as the OOM killer
/// kills a child; a later run returns.
#[cfg(test)]
pub(crate) fn kill_self_while(spec: &str) {
    let (n, dir) = spec.split_once(':').unwrap();
    let n: usize = n.parse().unwrap();
    let dir = std::path::Path::new(dir);
    let runs = std::fs::read_dir(dir).unwrap().count();
    if runs < n {
        std::fs::write(dir.join(runs.to_string()), b"").unwrap();
        kill_self();
    }
}

/// Ends this process by `SIGKILL`: it becomes a shell (`exec`, so the pid
/// stays) that sends the signal to itself. A `SIGKILL` ends a process the
/// same way whoever sends it; a helper process that sent it would outlive
/// the probe, and with no parent left to reap it stay a zombie under an
/// init that reaps none (the dev container's), which counts against its
/// limit on processes (fix pass 9).
#[cfg(test)]
pub(crate) fn kill_self() -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = Command::new("sh").args(["-c", "kill -KILL $$"]).exec();
        panic!("cannot exec sh to kill the probe: {e}");
    }
    #[cfg(not(unix))]
    panic!("no SIGKILL to send here");
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::{KILLED_RUNS, Marks, direct_runs, output, status};

    /// A shell that marks its run in `dir`; on each of its first `n` runs
    /// it is then killed by `SIGKILL` (it sends it to itself: a helper
    /// process that sent it would be left a zombie, `kill_self`), as the
    /// OOM killer would kill it, and after them it ends as `last` says.
    fn killed_while(n: usize, dir: &Path, last: &str) -> Command {
        let script = format!(
            "runs=$(( $(ls \"$1\" | wc -l) )); touch \"$1/$runs\"; \
             if [ \"$runs\" -lt {n} ]; then kill -KILL $$; sleep 60; exit 7; fi; {last}"
        );
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script).arg("sh").arg(dir);
        cmd
    }

    fn refused(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let p = std::panic::catch_unwind(f).expect_err("the run did not fail");
        p.downcast_ref::<String>().cloned().unwrap_or_default()
    }

    /// A child killed from outside once is run again, and its next run's
    /// outcome is the result (review round 8: one kill failed the test).
    #[test]
    fn a_child_killed_from_outside_once_is_run_again() {
        let marks = Marks::new("once");
        let o = output(&mut killed_while(1, &marks.0, "echo done; exit 3")).unwrap();
        assert_eq!(o.status.code(), Some(3), "{o:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), "done\n");
        assert_eq!(marks.runs(), 2);
        let marks = Marks::new("once-status");
        let s = status(&mut killed_while(1, &marks.0, "exit 0")).unwrap();
        assert!(s.success(), "{s:?}");
        assert_eq!(marks.runs(), 2);
    }

    /// Killed on each of its first `KILLED_RUNS - 1` runs, the child still
    /// comes to a result.
    #[test]
    fn a_child_killed_on_all_but_its_last_run_comes_to_a_result() {
        let marks = Marks::new("again");
        let n = usize::try_from(KILLED_RUNS - 1).unwrap();
        let o = output(&mut killed_while(n, &marks.0, "exit 0")).unwrap();
        assert!(o.status.success(), "{o:?}");
        assert_eq!(marks.runs(), n + 1);
    }

    /// A child killed on every run fails the test, saying the system
    /// killed it and how often, with its last output.
    #[test]
    fn a_child_killed_on_every_run_fails_saying_the_system_killed_it() {
        let marks = Marks::new("always");
        let n = usize::try_from(KILLED_RUNS).unwrap();
        let dir = marks.0.clone();
        let m = refused(move || {
            let mut cmd = killed_while(n, &dir, "exit 0");
            let _ = output(&mut cmd);
        });
        assert_eq!(marks.runs(), n);
        let want = format!("killed by SIGKILL on each of its {KILLED_RUNS} runs");
        assert!(m.contains(&want), "{m}");
        assert!(m.contains("the system ended it from outside"), "{m}");
        assert!(m.contains("the OOM killer"), "{m}");
        let marks = Marks::new("always-status");
        let dir = marks.0.clone();
        let m = refused(move || {
            let _ = status(&mut killed_while(n, &dir, "exit 0"));
        });
        assert_eq!(marks.runs(), n);
        assert!(m.contains(&want), "{m}");
    }

    /// Every ending that is the child's own comes back after one run: an
    /// exit status, an abort (`SIGABRT`), another signal it raises on
    /// itself.
    #[test]
    fn every_other_ending_is_the_first_run_result() {
        use std::os::unix::process::ExitStatusExt;
        for (script, code, signal) in [
            ("exit 0", Some(0), None),
            ("exit 101", Some(101), None),
            ("kill -ABRT $$", None, Some(6)),
            ("kill -SEGV $$", None, Some(11)),
            ("kill -TERM $$", None, Some(15)),
        ] {
            let marks = Marks::new("own");
            let mut cmd = Command::new("sh");
            cmd.arg("-c")
                .arg(format!("ulimit -c 0; touch \"$1/run\"; {script}"))
                .arg("sh")
                .arg(&marks.0);
            let o = output(&mut cmd).unwrap();
            assert_eq!(
                (o.status.code(), o.status.signal()),
                (code, signal),
                "{script}"
            );
            assert_eq!(marks.runs(), 1, "{script}: run again");
        }
    }

    /// A child that cannot start is the caller's error, as
    /// `Command::output`'s.
    #[test]
    fn a_child_that_cannot_start_is_an_error() {
        let mut cmd = Command::new("/nonexistent/unityrs-testkit-child");
        assert!(output(&mut cmd).is_err());
        assert!(status(&mut cmd).is_err());
    }

    /// Every child process a test in the workspace runs goes through this
    /// module (review round 8: the round-7 rule reached one runner only).
    #[test]
    fn every_child_in_the_workspace_runs_through_here() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let found = direct_runs(&crates);
        assert!(
            found.is_empty(),
            "children run outside unityrs_testkit::child, which a SIGKILL from \
             outside fails:\n{}",
            found.join("\n")
        );
    }

    /// The scan finds each way a test runs a child directly, and passes the
    /// runs through this module, a comment, and a dump error's status.
    #[test]
    fn the_scan_finds_each_direct_run() {
        let marks = Marks::new("scan");
        let file = |name: &str, text: &str| std::fs::write(marks.0.join(name), text).unwrap();
        file(
            "clean.rs",
            "let o = unityrs_testkit::child::output(Command::new(\"sh\").arg(\"x\"));\n\
             let s = child::status(&mut Command::new(\"sh\"));\n\
             let mut c = Command::new(\"sh\"); let p = child::live(scene, &mut c, false);\n\
             // Command::new(\"sh\").output() in a comment\n\
             assert_eq!(err.status(), 1);\n\
             std::thread::scope(|s| { s.spawn(|| ()); });\n",
        );
        file("output.rs", "let o = Command::new(\"sh\").output();\n");
        file(
            "status.rs",
            "let s = Command::new(\"sh\")\n    .arg(\"x\")\n    .status();\n",
        );
        file("spawn.rs", "let c = cmd.spawn();\n");
        file(
            "kill.rs",
            "child.kill();\nstd::process::Child::kill(&mut child);\n",
        );
        file("path.rs", "let o = Command::output(&mut cmd);\n");
        let found = direct_runs(&marks.0);
        let at = |name: &str| marks.0.join(name).display().to_string();
        assert_eq!(
            found,
            [
                format!("{}:1: child.kill();", at("kill.rs")),
                format!(
                    "{}:2: std::process::Child::kill(&mut child);",
                    at("kill.rs")
                ),
                format!(
                    "{}:1: let o = Command::new(\"sh\").output();",
                    at("output.rs")
                ),
                format!(
                    "{}: 1 commands made, 0 run through unityrs_testkit::child",
                    at("output.rs")
                ),
                format!("{}:1: let o = Command::output(&mut cmd);", at("path.rs")),
                format!("{}:1: let c = cmd.spawn();", at("spawn.rs")),
                format!(
                    "{}: 1 commands made, 0 run through unityrs_testkit::child",
                    at("status.rs")
                ),
            ]
        );
    }
}
