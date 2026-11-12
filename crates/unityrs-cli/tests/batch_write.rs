//! How `unityrs batch`'s main thread writes its two lines to a stdout and
//! a stderr another writer holds (`docs/BATCH_SPEC.md` section 10,
//! `extract.md` X8): one budget of 1 s for both, so a 78 to a full pipe, a
//! full pipe the process cannot open again, a full socket, and a full pipe
//! that is stdout and stderr both (`2>&1`) each exit within it, the line
//! dropped; a pipe the process cannot open again written through the
//! writer thread; `--check` with a past deadline; and SIGTTIN, SIGTTOU and
//! SIGXFSZ ignored while it writes (`batch.md` 2.11 step 1).

#![cfg(target_os = "linux")]

mod common;

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{INPUTS, check_to, enterable, fill, fill_file, job, read_all, write_job};
use serde_json::Value;
use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;

/// The budget both lines share.
const BUDGET: Duration = Duration::from_secs(1);
/// The most a held run may take: the budget, the process's start and end.
const BOUND: Duration = Duration::from_secs(3);

fn none_json(dir: &Scratch) -> std::path::PathBuf {
    dir.join("none.json")
}

/// X8: a 78 to a pipe another writer keeps full: its line dropped when the
/// budget is spent, the exit status the record.
#[test]
fn x8_a_78_to_a_full_pipe_is_dropped_after_a_second() {
    let dir = Scratch::new("cli-batch-full");
    enterable(&dir);
    let (r, w) = std::io::pipe().unwrap();
    let other = fill(&r);
    let (code, took) = check_to(&none_json(&dir), Stdio::from(w), Stdio::null());
    assert_eq!(code, Some(78));
    assert!(took >= BUDGET, "{took:?}");
    assert!(took < BOUND, "{took:?}");
    drop(other);
    assert!(
        read_all(r).iter().all(|&b| b == b'x'),
        "the line was dropped"
    );
}

/// A pipe `fchmod`ed to 0000, which the process cannot open again (as a
/// pipe another uid made; a test run as root runs the binary as `nobody`).
fn locked_pipe() -> (std::io::PipeReader, File) {
    let (r, w) = std::io::pipe().unwrap();
    let w = File::from(OwnedFd::from(w));
    (r, w)
}

fn lock(w: &File) {
    w.set_permissions(std::fs::Permissions::from_mode(0o000))
        .unwrap();
}

/// X8: a 78 to a pipe the process cannot open again: its line through the
/// writer thread, within the budget.
#[test]
fn x8_a_78_to_a_pipe_that_cannot_be_reopened_is_written_by_the_thread() {
    let dir = Scratch::new("cli-batch-locked");
    enterable(&dir);
    let (r, w) = locked_pipe();
    lock(&w);
    let (code, took) = check_to(&none_json(&dir), Stdio::from(w), Stdio::null());
    assert_eq!(code, Some(78));
    assert!(took < BOUND, "{took:?}");
    let got = String::from_utf8(read_all(r)).unwrap();
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["detail"], "unreadable");
}

/// X8: the same pipe, full: the writer thread is waited on for the budget
/// alone, the line dropped.
#[test]
fn x8_a_78_to_a_full_pipe_that_cannot_be_reopened_is_dropped() {
    let dir = Scratch::new("cli-batch-locked-full");
    enterable(&dir);
    let (r, w) = locked_pipe();
    // Filled while it can still be opened again, then locked.
    let other = fill(&w);
    lock(&w);
    let (code, took) = check_to(&none_json(&dir), Stdio::from(w), Stdio::null());
    assert_eq!(code, Some(78));
    assert!(took >= BUDGET, "{took:?}");
    assert!(took < BOUND, "{took:?}");
    drop(other);
    assert!(
        read_all(r).iter().all(|&b| b == b'x'),
        "the line was dropped"
    );
}

/// X8: a 78 to a full socket (a socketpair's end, which `/proc/self/fd`
/// cannot open again): the writer thread, waited on for the budget, the
/// line dropped.
#[test]
fn x8_a_78_to_a_full_socket_is_dropped_after_a_second() {
    let dir = Scratch::new("cli-batch-socket");
    enterable(&dir);
    let (ours, theirs) = UnixStream::pair().unwrap();
    theirs.set_nonblocking(true).unwrap();
    let as_file = File::from(OwnedFd::from(theirs.try_clone().unwrap()));
    fill_file(&as_file);
    drop(as_file);
    // The description is the child's too: blocking, as a caller's would be.
    theirs.set_nonblocking(false).unwrap();
    let (code, took) = check_to(
        &none_json(&dir),
        Stdio::from(OwnedFd::from(theirs)),
        Stdio::null(),
    );
    assert_eq!(code, Some(78));
    assert!(took >= BUDGET, "{took:?}");
    assert!(took < BOUND, "{took:?}");
    assert!(
        read_all(ours).iter().all(|&b| b == b'x'),
        "the line was dropped"
    );
}

/// X8: one full pipe as stdout and stderr (`2>&1`): both lines share the
/// one budget, so the run takes about 1 s, not 2.
#[test]
fn x8_a_78_to_one_full_pipe_for_both_lines_takes_one_budget() {
    let dir = Scratch::new("cli-batch-both");
    enterable(&dir);
    let (r, w) = std::io::pipe().unwrap();
    let other = fill(&r);
    let w2 = w.try_clone().unwrap();
    let (code, took) = check_to(&none_json(&dir), Stdio::from(w), Stdio::from(w2));
    assert_eq!(code, Some(78));
    assert!(took >= BUDGET, "{took:?}");
    assert!(
        took < BUDGET + Duration::from_millis(900),
        "one budget for both lines: {took:?}"
    );
    drop(other);
    assert!(read_all(r).iter().all(|&b| b == b'x'), "both lines dropped");
}

/// X8: `--check` sets no deadline, so a past one still has its line
/// written to a pipe; and a 78 whose `JOB.json` names a past deadline has
/// none either (a refused spec has no deadline).
#[test]
fn x8_a_past_deadline_holds_no_line_back() {
    let dir = Scratch::new("cli-batch-deadline");
    enterable(&dir);
    let past = r#","deadline":"2001-01-01T00:00:00Z""#;
    let p = write_job(&dir, &job(&format!("{INPUTS}{past}")));
    let (r, w) = std::io::pipe().unwrap();
    assert_eq!(check_to(&p, Stdio::from(w), Stdio::null()).0, Some(0));
    let v: Value = serde_json::from_slice(&read_all(r)).unwrap();
    assert!(v["detail"].as_str().unwrap().starts_with("memory_bound "));
    let bad = job(&format!("{INPUTS}{past}")).replace(r#""threads":2"#, r#""threads":0"#);
    let p = write_job(&dir, &bad);
    let (r, w) = std::io::pipe().unwrap();
    assert_eq!(check_to(&p, Stdio::from(w), Stdio::null()).0, Some(78));
    let v: Value = serde_json::from_slice(&read_all(r)).unwrap();
    assert_eq!(v["detail"], "range /threads");
}

/// SIGTTIN, SIGTTOU and SIGXFSZ, as bits of `/proc/<pid>/status`'s
/// `SigIgn` (signal `n` is bit `n - 1`).
const JOB_CONTROL: u64 =
    (1 << (libc::SIGTTIN - 1)) | (1 << (libc::SIGTTOU - 1)) | (1 << (libc::SIGXFSZ - 1));

/// Runs the binary in the background of a shell, its stdout the full pipe
/// the shell has, so it waits out its budget, and prints its `SigIgn`
/// every 10 ms while it lives, after the shell's own.
const WATCH: &str = r#"
"$0" batch --check "$1" & pid=$!
if [ -n "${2-}" ]; then
  echo attempt >> "$2/attempts"
  if [ ! -f "$2/killed" ]; then
    touch "$2/killed"
    kill -s KILL "$pid"
  fi
fi
echo "shell $(sed -n 's/^SigIgn:[[:space:]]*//p' /proc/$$/status)" >&2
i=0
while [ $i -lt 300 ]; do
  case "$(sed -n 's/^State:[[:space:]]*//p' /proc/$pid/status 2>/dev/null)" in
    ''|Z*) break ;;
  esac
  echo "child $(sed -n 's/^SigIgn:[[:space:]]*//p' /proc/$pid/status 2>/dev/null)" >&2
  i=$((i + 1))
  sleep 0.01
done
wait $pid
status=$?
if [ "$status" -gt 128 ]; then kill -s KILL $$; fi
echo "exit $status" >&2
"#;

/// `batch.md` 2.11 step 1: before it writes, the engine sets SIGTTIN,
/// SIGTTOU and SIGXFSZ to `SIG_IGN`, so a write to a terminal from a
/// background process group (`tostop`) cannot stop it. Asserted in the
/// child, by its `SigIgn` while it waits on a full stdout, against a shell
/// that ignores none of the three.
#[test]
fn the_engine_ignores_job_control_signals_before_it_writes() {
    watch_engine(false);
}

#[test]
fn a_killed_engine_retries_the_whole_signal_watch() {
    watch_engine(true);
}

fn watch_engine(kill_once: bool) {
    let dir = Scratch::new("cli-batch-sigign");
    enterable(&dir);
    let marks = dir.join("marks");
    std::fs::create_dir(&marks).unwrap();
    // A root-run test's unprivileged shell must be able to record its
    // attempts too; all of these files stay under the parent's scratch.
    std::fs::set_permissions(&marks, std::fs::Permissions::from_mode(0o777)).unwrap();
    let report = dir.join("report");
    let (r, w) = std::io::pipe().unwrap();
    let other = fill(&r);
    let status = child::status(common::unprivileged(
        Command::new("sh")
            .arg("-c")
            .arg(WATCH)
            .arg(common::BIN)
            .arg(none_json(&dir))
            .arg(if kill_once {
                marks.as_os_str()
            } else {
                std::ffi::OsStr::new("")
            })
            .stdout(Stdio::from(w))
            .stderr(Stdio::from(File::create(&report).unwrap())),
    ))
    .unwrap();
    assert!(status.success());
    drop(other);
    drop(r);
    let text = std::fs::read_to_string(&report).unwrap();
    let mask = |l: &str| u64::from_str_radix(l.trim(), 16).ok();
    let shell = text
        .lines()
        .find_map(|l| l.strip_prefix("shell "))
        .and_then(mask)
        .expect("the shell's mask");
    assert_eq!(shell & JOB_CONTROL, 0, "the shell ignores none: {text}");
    let child: Vec<u64> = text
        .lines()
        .filter_map(|l| l.strip_prefix("child "))
        .filter_map(mask)
        .collect();
    assert!(
        child.iter().any(|m| m & JOB_CONTROL == JOB_CONTROL),
        "the engine ignores all three while it writes: {text}"
    );
    assert!(text.contains("exit 78"), "{text}");
    if kill_once {
        let attempts = std::fs::read_to_string(marks.join("attempts")).unwrap();
        assert_eq!(attempts.lines().count(), 2, "the killed scene was retried");
    }
    let _ = std::io::stderr().flush();
}
