//! The borrowed writer on pipes and sockets of the test's own: each line has its own
//! try, so one stream's failure never costs another its line, and each
//! line's outcome is read as the wait ends: written, failed, or unresolved
//! while its write has not been marked returned.
use super::{Outcome, raw_line, write, write_with};
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use unityrs_testkit::guard;

/// stdout failing at once (`EPIPE`, a socket whose writes are shut down,
/// which no other process's copy of a descriptor can hold open, as a child
/// forked beside this test holds a pipe's read end) and stderr writable:
/// stderr still gets its line, before the wait ends, and the one line not
/// written is a failure.
#[test]
fn a_failing_stdout_leaves_a_writable_stderr_its_line() {
    let (out_w, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    out_w.shutdown(std::net::Shutdown::Write).unwrap();
    let (mut err_r, err_w) = std::io::pipe().unwrap();
    let budget = Duration::from_secs(10);
    let started = Instant::now();
    let outcome = write(
        &[(out_w.as_fd(), b"exit\n"), (err_w.as_fd(), b"err\n")],
        started + budget,
        None,
    );
    let took = started.elapsed();
    drop(err_w);
    let mut got = Vec::new();
    err_r.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"err\n", "stderr's line");
    let want = Outcome {
        written: 1,
        failed: 1,
        unresolved: 0,
    };
    assert_eq!(outcome, want, "stdout's line, failed");
    assert!(took < budget, "returned once both were tried: {took:?}");
}

/// Both written: none failed or unresolved.
#[test]
fn lines_written_whole_count_none() {
    let (mut out_r, out_w) = std::io::pipe().unwrap();
    let (mut err_r, err_w) = std::io::pipe().unwrap();
    let outcome = write(
        &[(out_w.as_fd(), b"exit\n"), (err_w.as_fd(), b"err\n")],
        Instant::now() + Duration::from_secs(10),
        None,
    );
    assert_eq!(
        outcome,
        Outcome {
            written: 2,
            ..Outcome::default()
        }
    );
    drop((out_w, err_w));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    out_r.read_to_end(&mut out).unwrap();
    err_r.read_to_end(&mut err).unwrap();
    assert_eq!(
        (out.as_slice(), err.as_slice()),
        (&b"exit\n"[..], &b"err\n"[..])
    );
}

/// A line written whole whose write has not been marked returned when the
/// wait ends (its thread held between the two, as a preemption would):
/// unresolved, never a failure, though it is in the pipe. The wait ends
/// once the write has returned, whenever the thread got there.
#[test]
fn a_line_written_but_not_marked_is_unresolved_never_failed() {
    let (mut r, w) = std::io::pipe().unwrap();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (wrote_tx, wrote_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    let outcome = write_with(
        &[(w.as_fd(), b"exit\n")],
        Instant::now() + Duration::from_secs(10),
        None,
        move |fd, line, until| {
            let whole = raw_line(fd, line, until);
            wrote_tx.send(()).unwrap();
            guard::hold(&go_rx, "the test to let the write's mark go");
            done_tx.send(()).unwrap();
            whole
        },
        |_| guard::wait(&wrote_rx, "the line's write to return"),
    );
    let want = Outcome {
        unresolved: 1,
        ..Outcome::default()
    };
    assert_eq!(outcome, want);
    go_tx.send(()).unwrap();
    guard::wait(&done_rx, "the writer thread's end");
    drop(w);
    let mut got = Vec::new();
    r.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"exit\n", "written all the same");
}

/// stdout's write held through the wait's end, and stderr's line behind
/// it: stdout's unresolved, stderr's failed, and once the hold ends that
/// line indeed gives up before any write.
#[test]
fn a_held_write_is_unresolved_and_the_line_behind_it_failed() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    };
    let (_out_r, out_w) = std::io::pipe().unwrap();
    let (mut err_r, err_w) = std::io::pipe().unwrap();
    let out_fd = std::os::fd::AsRawFd::as_raw_fd(&out_w);
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    let wrote = Arc::new(AtomicBool::new(false));
    let marked = wrote.clone();
    let (held_tx, held_rx) = mpsc::channel::<()>();
    // `until` already past: stderr's try, once the hold ends, gives up at
    // its first check; the wait ends once stdout's write is in its try.
    let outcome = write_with(
        &[(out_w.as_fd(), b"exit\n"), (err_w.as_fd(), b"err\n")],
        Instant::now(),
        None,
        move |fd, line, until| {
            if fd == out_fd {
                held_tx.send(()).unwrap();
                guard::hold(&go_rx, "the test to let stdout's write return");
                return false;
            }
            let whole = raw_line(fd, line, until);
            marked.store(whole, SeqCst);
            done_tx.send(()).unwrap();
            whole
        },
        |_| guard::wait(&held_rx, "stdout's write, held in its try"),
    );
    let want = Outcome {
        written: 0,
        failed: 1,
        unresolved: 1,
    };
    assert_eq!(outcome, want);
    go_tx.send(()).unwrap();
    guard::wait(&done_rx, "stderr's try, past the wait");
    assert!(!wrote.load(SeqCst), "stderr's line written");
    drop(err_w);
    let mut got = Vec::new();
    err_r.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"", "nothing written behind the held line");
}
