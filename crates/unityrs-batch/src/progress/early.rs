//! The main thread's own lines (`docs/plans/batch.md` 2.11, *The main
//! thread's `exit` line*; `docs/BATCH_SPEC.md` section 10): the `exit`
//! line on stdout, then a line on stderr, written while no progress thread
//! exists, both within one bound.
//!
//! - [`write`], **without a deadline** (phase 1's 78s, `--check`, a start
//!   that fails before validation accepts a spec): both lines share one
//!   budget of [`OWN_WAIT`] from the call, each written in what the one
//!   before it left. To a regular file or a block device a line is written
//!   as it is (nothing bounds the host's writeback, section 10). To a
//!   pipe, a FIFO or a terminal it goes through a description of its own,
//!   the descriptor opened again as `/proc/self/fd/<n>` with `O_WRONLY |
//!   O_NONBLOCK | O_CLOEXEC | O_NOCTTY`, and the rest of it is dropped
//!   when the budget is spent. When that reopen fails (`EACCES`, a pipe
//!   another uid created), on a socket or a character device other than
//!   a terminal, and off Linux, it goes through a writer thread, waited
//!   on for what is left of the budget. So another
//!   writer of the same pipe holds the two lines for at most that second.
//! - [`within`], **with a deadline** (later PRs' 69s): both lines through
//!   the writer thread, whatever the descriptors are, waited on for at
//!   most the time given; none when it is zero.
//!
//! A full non-blocking description is tried again every [`RETRY`] until
//! the budget is spent, and a socket takes the writer thread, in place of
//! `poll` and `send(..., MSG_DONTWAIT)`: the same bound, in safe code.
//! Every try, whether it wrote part of the line, was interrupted or found
//! the pipe full, checks the budget first.
//!
//! `WouldBlock` on the writer thread also retries every [`RETRY`] until
//! the shared deadline. Other write errors drop the line: the exit status
//! is the record.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, IsTerminal, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long, without a deadline, the main thread gives its two lines.
pub const OWN_WAIT: Duration = Duration::from_secs(1);

/// The pause between two tries of a full non-blocking description.
pub const RETRY: Duration = Duration::from_millis(1);

/// The writer thread's stack (`docs/plans/batch.md` 2.11).
const WRITER_STACK: usize = 2 << 20;

/// Writes `out` on stdout, then `err` on stderr, without a deadline, both
/// within [`OWN_WAIT`] of the call (module docs). Either may be empty.
pub fn write(out: &[u8], err: &[u8]) {
    write_on([(io::stdout().as_fd(), out), (io::stderr().as_fd(), err)]);
}
pub(crate) fn write_on<const N: usize>(lines: [(BorrowedFd<'_>, &[u8]); N]) {
    let until = Instant::now() + OWN_WAIT;
    write_on_until(lines, until);
}
pub(crate) fn write_on_until<const N: usize>(lines: [(BorrowedFd<'_>, &[u8]); N], until: Instant) {
    for (fd, line) in lines {
        own(fd, line, until);
    }
}

/// Writes `out` on stdout, then `err` on stderr, through the writer
/// thread, waited on for at most `wait`; with a zero `wait`, nothing is
/// written. Gives whether both writes returned in time.
#[must_use]
pub fn within(out: &[u8], err: &[u8], wait: Duration) -> bool {
    within_on(
        [(io::stdout().as_fd(), out), (io::stderr().as_fd(), err)],
        wait,
    )
}

/// [`within`] on the descriptors given, in order.
pub(crate) fn within_on<const N: usize>(
    lines: [(BorrowedFd<'_>, &[u8]); N],
    wait: Duration,
) -> bool {
    if wait.is_zero() {
        return false;
    }
    let until = Instant::now() + wait;
    let mut owned = Vec::new();
    for (fd, line) in lines.iter().filter(|(_, l)| !l.is_empty()) {
        let Ok(dup) = fd.try_clone_to_owned() else {
            // A full descriptor table: both lines, in order, on the
            // borrowed descriptors, on a thread, within the same wait.
            drop(owned);
            return borrowed(&lines, until);
        };
        owned.push((File::from(dup), line.to_vec()));
    }
    through_thread(owned, wait)
}

/// Lines no description of their own could be made for: on the borrowed
/// descriptors, on a thread, by `until`, each tried whatever another's
/// write did. Each one not written whole when the wait ends is counted
/// ([`dropped`]): one that failed, never began or had no thread, and one
/// still in its write, which may yet land but which nothing will vouch
/// for (2.11). Gives whether every one was written.
fn borrowed(lines: &[(BorrowedFd<'_>, &[u8])], until: Instant) -> bool {
    let outcome = crate::sys::borrowed::write(lines, until, ON_START.get().copied());
    let left = outcome.failed.saturating_add(outcome.unresolved);
    DROPPED.fetch_add(
        u64::try_from(left).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::SeqCst,
    );
    left == 0
}
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The main thread's early lines a full descriptor table dropped, each
/// counted (the head's `stderr_dropped`).
#[must_use]
pub fn dropped() -> u64 {
    DROPPED.load(std::sync::atomic::Ordering::SeqCst)
}

/// One line on `fd`, by `until`.
fn own(fd: BorrowedFd<'_>, line: &[u8], until: Instant) {
    own_with_reopen(fd, line, until, reopen);
}

/// `own` with the reopen operation supplied, so tests can observe which
/// descriptors need a separate description without replacing the writer.
fn own_with_reopen(
    fd: BorrowedFd<'_>,
    line: &[u8],
    until: Instant,
    reopen: impl FnOnce(BorrowedFd<'_>) -> Option<File>,
) {
    if line.is_empty() {
        return;
    }
    let Ok(dup) = fd.try_clone_to_owned() else {
        // A full descriptor table: the borrowed descriptor, on a thread,
        // never waited on past the budget.
        let _ = borrowed(&[(fd, line)], until);
        return;
    };
    let file = File::from(dup);
    let Ok(meta) = file.metadata() else {
        return;
    };
    let kind = meta.file_type();
    if kind.is_file() || kind.is_block_device() {
        let _ = (&file).write_all(line);
        return;
    }
    let left = until.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return;
    }
    if (kind.is_fifo() || file.is_terminal())
        && let Some(own) = reopen(fd)
    {
        nonblocking(&own, line, until);
        return;
    }
    let _ = through_thread(
        vec![(file, line.to_vec())],
        until.saturating_duration_since(Instant::now()),
    );
}

#[cfg(test)]
#[path = "own_tests.rs"]
mod own_tests;

/// `fd` opened again, non-blocking, as `/proc/self/fd/<n>`: a description
/// of its own, so its flags are not the caller's.
fn reopen(fd: BorrowedFd<'_>) -> Option<File> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .ok()
}

/// Writes `line` until it is written, a write fails, or `until` passes,
/// checked before every try. A blocking file may hold one try past the
/// budget; on the writer thread only the main thread's wait is bounded.
pub(crate) fn nonblocking(file: &File, line: &[u8], until: Instant) {
    let mut rest = line;
    let mut w = file;
    while !rest.is_empty() {
        if Instant::now() >= until {
            return;
        }
        match w.write(rest) {
            Ok(0) => return,
            Ok(n) => rest = rest.get(n..).unwrap_or_default(),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(RETRY),
            Err(_) => return,
        }
    }
}

/// What the writer thread calls first: the process host's thread
/// registration (the `tally` hook's), set once.
static ON_START: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
/// Set the writer thread's [`ON_START`] (the process host, at its start).
pub fn on_thread_start(register: fn()) {
    let _ = ON_START.set(register);
}
/// Writes each line on its file, in order, on a writer thread of its own,
/// and waits for it at most `wait`: gives whether it finished in time. A
/// thread that cannot be spawned writes nothing; one still writing when
/// the wait ends is left to the process's exit.
pub(crate) fn through_thread(lines: Vec<(File, Vec<u8>)>, wait: Duration) -> bool {
    if lines.is_empty() {
        return true;
    }
    let until = Instant::now() + wait;
    let (tx, rx) = mpsc::sync_channel::<()>(1);
    let spawned = std::thread::Builder::new()
        .name("exit-writer".into())
        .stack_size(WRITER_STACK)
        .spawn(move || {
            if let Some(register) = ON_START.get() {
                register();
            }
            for (f, l) in lines {
                nonblocking(&f, &l, until);
            }
            let _ = tx.send(());
        });
    if spawned.is_err() {
        return false;
    }
    rx.recv_timeout(until.saturating_duration_since(Instant::now()))
        .is_ok()
}
