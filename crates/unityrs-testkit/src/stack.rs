//! Stack probes: how a test finds the least stack a path needs, and tells a
//! stack overflow from every other way a run can fail.
//!
//! A thread that overflows its stack takes its whole process with it, so a
//! test that bisects for the least stack runs the path once per size in a
//! child process: this test binary again, running one ignored probe test,
//! with the size in an environment variable ([`fits`], [`least`]). Only a
//! stack overflow says a size is too small. Rust's runtime catches the
//! fault on a thread's guard page, prints `thread '…' has overflowed its
//! stack` and aborts the process (`SIGABRT`). Anything else a child does is
//! no measurement, and fails the test with what happened: a panic (exit
//! code 101), an abort with no overflow report (an allocation that failed),
//! another signal (`SIGKILL`, the OOM killer under memory pressure; a
//! `SIGSEGV` the runtime did not report, so not on a guard page), or a
//! probe that did not run (libtest passes a filter that matches nothing).
//! (Review round 5: three tests read every unsuccessful child as an
//! overflow, so a child killed under memory pressure moved a measurement,
//! and a check made from one could fail, or pass, on a wrong figure.)
//!
//! Owner: package B.

use std::process::{Command, ExitStatus, Output};

/// What Rust's runtime prints when a thread's stack overflows.
pub const OVERFLOW_REPORT: &str = "has overflowed its stack";

/// What one run of a probe came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The probe ran and passed.
    Fits,
    /// A thread overflowed its stack.
    Overflows,
}

/// What a child that exited with `status`, printing `stdout` and `stderr`,
/// came to, or what else happened to it.
///
/// # Errors
/// Every outcome but a passed probe and a stack overflow, described.
pub fn classify(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> Result<Probe, String> {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let tail = || format!("\n--- stdout\n{}\n--- stderr\n{}", last(&out), last(&err));
    if status.success() {
        return if out.contains("test result: ok. 1 passed;") {
            Ok(Probe::Fits)
        } else {
            Err(format!("the probe did not run (no test matched){}", tail()))
        };
    }
    let reported = err.contains(OVERFLOW_REPORT);
    match (signal(status), status.code()) {
        (Some(ABORT), _) if reported => Ok(Probe::Overflows),
        (Some(ABORT), _) => Err(format!(
            "the probe aborted with no stack overflow reported (an allocation that failed \
             prints \"memory allocation of N bytes failed\"){}",
            tail()
        )),
        (Some(KILL), _) => Err(format!(
            "the probe was killed (SIGKILL: the OOM killer, or a person){}",
            tail()
        )),
        (Some(s), _) => Err(format!(
            "the probe died of signal {s}, with{} a stack overflow reported{}",
            if reported { "" } else { "out" },
            tail()
        )),
        (None, Some(101)) => Err(format!("the probe panicked{}", tail())),
        (None, code) => Err(format!("the probe exited with {code:?}{}", tail())),
    }
}

const ABORT: i32 = 6;
const KILL: i32 = 9;

#[cfg(unix)]
fn signal(status: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&status)
}

#[cfg(not(unix))]
fn signal(_: ExitStatus) -> Option<i32> {
    None
}

/// The last lines of `text`, enough to say what a child did.
fn last(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// Runs this test binary's ignored test `probe` (its whole path, as
/// `--exact` matches it) in a child process with `var` set to `value`.
///
/// # Panics
/// When the child can not start, or comes to anything but [`Probe`]
/// ([`classify`]): the message says what, and ends with the child's last
/// lines of output.
#[track_caller]
#[must_use]
pub fn run(probe: &str, var: &str, value: &str) -> Probe {
    let exe = std::env::current_exe().expect("the test binary's path");
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(exe)
        .args(["--exact", probe, "--ignored", "--test-threads=1"])
        .env(var, value)
        .output()
        .unwrap_or_else(|e| panic!("cannot start probe {probe}: {e}"));
    classify(status, &stdout, &stderr)
        .unwrap_or_else(|e| panic!("probe {probe} with {var}={value:?}: {e}"))
}

/// Whether `probe` passes with `var` set to `value`, and not whether it
/// overflowed a stack ([`run`]).
///
/// # Panics
/// As [`run`].
#[track_caller]
#[must_use]
pub fn fits(probe: &str, var: &str, value: &str) -> bool {
    run(probe, var, value) == Probe::Fits
}

/// The least stack, to 4 KiB, of those up to `hi` on which `fits` holds,
/// by bisection: `fits` is false only for a stack overflow ([`fits`]).
///
/// # Panics
/// When `hi` itself does not fit, naming `what`.
#[track_caller]
pub fn least(hi: usize, what: &str, mut fits: impl FnMut(usize) -> bool) -> usize {
    assert!(fits(hi), "{what} overflows {hi} bytes");
    let (mut lo, mut hi) = (0, hi);
    while hi - lo > 4 << 10 {
        let mid = usize::midpoint(lo, hi) / 4096 * 4096;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[cfg(test)]
mod tests {
    use super::{Probe, classify, least, run};

    const VAR: &str = "UNITYRS_TESTKIT_STACK_PROBE";

    /// Run by the tests below: on a thread of 256 KiB, `recurse` recurses
    /// without end, `panic` panics, `alloc` fails an allocation, and `pass`
    /// returns.
    #[test]
    #[ignore = "run by the stack probe tests"]
    fn probe() {
        let Ok(what) = std::env::var(VAR) else {
            return;
        };
        let t = std::thread::Builder::new().stack_size(256 << 10);
        t.spawn(move || match what.as_str() {
            "recurse" => {
                std::hint::black_box(recurse(0));
            }
            "panic" => panic!("the probe's own panic"),
            "alloc" => {
                std::alloc::handle_alloc_error(
                    std::alloc::Layout::from_size_align(1 << 40, 8).unwrap(),
                );
            }
            _ => {}
        })
        .unwrap()
        .join()
        .unwrap();
    }

    fn recurse(n: u64) -> u64 {
        if std::hint::black_box(n) == u64::MAX {
            return 0;
        }
        let pad = std::hint::black_box([n; 64]);
        recurse(n + 1).wrapping_add(pad[0])
    }

    const PROBE: &str = "stack::tests::probe";

    fn refused(what: &str) -> String {
        let r = std::panic::catch_unwind(|| run(PROBE, VAR, what));
        let p = r.expect_err(what);
        p.downcast_ref::<String>().cloned().unwrap_or_default()
    }

    /// A run that passes fits; one that recurses without end overflows.
    #[test]
    fn a_pass_fits_and_a_stack_overflow_overflows() {
        assert_eq!(run(PROBE, VAR, "pass"), Probe::Fits);
        assert_eq!(run(PROBE, VAR, "recurse"), Probe::Overflows);
    }

    /// A panic, a failed allocation and a probe that did not run are no
    /// measurement: each fails the test, naming what happened, where a
    /// child's failure alone read them as overflows (review round 5).
    #[test]
    fn every_other_failure_is_no_measurement() {
        let m = refused("panic");
        assert!(m.contains("the probe panicked"), "{m}");
        assert!(m.contains("the probe's own panic"), "{m}");
        let m = refused("alloc");
        assert!(m.contains("aborted with no stack overflow reported"), "{m}");
        assert!(m.contains("memory allocation of"), "{m}");
        let r = std::panic::catch_unwind(|| run("stack::tests::no_such_probe", VAR, "pass"));
        let m = r.expect_err("no such probe");
        let m = m.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(m.contains("the probe did not run"), "{m}");
    }

    /// The other signals, and an overflow reported under one: SIGKILL (the
    /// OOM killer) and SIGSEGV off a guard page are failures; only SIGABRT
    /// with the runtime's report is an overflow.
    #[cfg(unix)]
    #[test]
    fn only_an_abort_with_the_overflow_report_is_an_overflow() {
        use std::os::unix::process::ExitStatusExt;
        let report =
            b"\nthread 'w' has overflowed its stack\nfatal runtime error: stack overflow\n";
        let of = |raw: i32, err: &[u8]| classify(ExitStatusExt::from_raw(raw), b"", err);
        assert_eq!(of(6, report), Ok(Probe::Overflows));
        assert!(
            of(6, b"")
                .unwrap_err()
                .contains("no stack overflow reported")
        );
        assert!(of(9, report).unwrap_err().contains("SIGKILL"));
        assert!(of(11, b"").unwrap_err().contains("signal 11"));
        assert!(of(101 << 8, report).unwrap_err().contains("panicked"));
        assert!(of(0, b"").unwrap_err().contains("did not run"));
        let passed = b"test result: ok. 1 passed; 0 failed; 0 ignored";
        assert_eq!(
            classify(ExitStatusExt::from_raw(0), passed, b""),
            Ok(Probe::Fits)
        );
    }

    /// [`least`] finds the least size that fits, to 4 KiB.
    #[test]
    fn least_bisects_to_four_kib() {
        let need = 1_234_567;
        let got = least(8 << 20, "a model", |s| s >= need);
        assert!(got >= need && got - need < 4 << 10, "{got}");
    }
}
