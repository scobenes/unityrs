//! Deadlock guards: how a test that hands work between threads turns a
//! hang into a failure without timing the code it tests.
//!
//! A test of a lock rule (a callback that re-enters the core, a thread that
//! holds a lock another thread's callback needs, a rendezvous) hangs for
//! ever when the code under test deadlocks or never ends, so it waits with
//! a limit and fails past it. The limit is no measure of the code: a
//! correct run answers at once, and [`DEADLOCK_GUARD`] is ten minutes,
//! beyond any delay a loaded machine's scheduler adds, so only a hang
//! reaches it, and each failure says that it is this guard. A hook that
//! holds a thread at a point until the test lets it go ([`hold`]) waits as
//! long, so load never lets it go early. [`unguarded`] finds a timed wait
//! that does not take its limit from here (review round 3: guards of 10 s
//! could fail on a loaded machine).

use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// How long a deadlock guard waits: ten minutes.
pub const DEADLOCK_GUARD: Duration = Duration::from_secs(600);

/// `rx`'s next message, waited for at most [`DEADLOCK_GUARD`]; `what` names
/// what the test waits for, and what a lock held where it must not be
/// would deadlock.
///
/// # Panics
/// Past the guard with no message, or when every sender is gone without
/// sending (the thread that held one panicked, and its own message says
/// why).
#[track_caller]
#[must_use]
pub fn recv<T>(rx: &Receiver<T>, what: &str) -> T {
    match rx.recv_timeout(DEADLOCK_GUARD) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => hung(what),
        Err(RecvTimeoutError::Disconnected) => {
            panic!(
                "the thread that would answer panicked without answering (its own message says \
                 why); waiting for {what}"
            )
        }
    }
}

/// Waits for a signal on `rx` (a thread has reached a point), as [`recv`].
///
/// # Panics
/// As [`recv`].
#[track_caller]
pub fn wait(rx: &Receiver<()>, what: &str) {
    let () = recv(rx, what);
}

/// Holds the calling thread (a test's hook) until the test lets it go, a
/// message on `rx`, or drops the sender (it is done with the thread).
///
/// # Panics
/// Past [`DEADLOCK_GUARD`] with neither: the test is waiting on this thread.
#[track_caller]
pub fn hold(rx: &Receiver<()>, what: &str) {
    if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(DEADLOCK_GUARD) {
        hung(what);
    }
}

#[track_caller]
fn hung(what: &str) -> ! {
    panic!(
        "deadlock guard: no answer in {} s, which only a hang takes, not load; waiting for {what}",
        DEADLOCK_GUARD.as_secs()
    )
}

/// Runs `f` on a thread of its own and returns what it returns, failing as
/// [`recv`] does; past the guard the thread is left behind, not waited for.
///
/// # Panics
/// As [`recv`].
#[track_caller]
#[must_use]
pub fn finishes<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    recv(&rx, what)
}

/// The calls that wait with a time limit: a channel's, a condition
/// variable's, a park's.
pub const TIMED_WAITS: &[&str] = &[
    ".recv_timeout(",
    ".wait_timeout(",
    ".wait_timeout_while(",
    "::park_timeout(",
];

/// Every `file:line` of a `.rs` file under `root` (`target` directories
/// left out) that calls a [`TIMED_WAITS`] wait in a file that never names
/// [`DEADLOCK_GUARD`], whose limit therefore comes from somewhere else.
/// Comment lines are not scanned.
///
/// # Panics
/// When `root` cannot be read (a test helper).
#[must_use]
pub fn unguarded(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if text.contains("DEADLOCK_GUARD") {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if !line.trim_start().starts_with("//") && TIMED_WAITS.iter().any(|t| line.contains(t))
            {
                found.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            if path.file_name().is_none_or(|n| n != "target") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{finishes, hold, recv, unguarded, wait};

    /// Every crate's timed waits take their limit from [`super::DEADLOCK_GUARD`].
    #[test]
    fn every_timed_wait_in_the_workspace_takes_the_guard() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let found = unguarded(&crates);
        assert!(
            found.is_empty(),
            "timed waits with a limit of their own, which load can outlast:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn an_answer_comes_back() {
        assert_eq!(finishes("a sum", || 1 + 1), 2);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(3).unwrap();
        assert_eq!(recv(&rx, "a sent value"), 3);
    }

    #[test]
    #[should_panic(expected = "panicked without answering")]
    fn a_thread_that_panics_fails_at_once_and_says_so() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        drop(tx);
        wait(&rx, "a signal");
    }

    #[test]
    fn a_hold_ends_when_the_test_lets_it_go_or_drops_the_sender() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(()).unwrap();
        hold(&rx, "the test to let go");
        drop(tx);
        hold(&rx, "the test to drop its sender");
    }
}
