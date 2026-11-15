//! The main thread's early lines on descriptors it could not duplicate
//! (`docs/plans/batch.md` 2.11's *The main thread's `exit` line*).
use std::io;

/// What became of [`write`]'s lines by the time its wait ended, each line
/// in one count alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Written whole.
    pub written: usize,
    /// Never to be written whole: its try failed or gave up at `until`, or
    /// had not begun when the wait ended, so it begins past `until` and
    /// gives up before its first write; or no thread was spawned.
    pub failed: usize,
    /// In its try when the wait ended (one line at most): written whole
    /// already, still to be, or never; nothing can tell which.
    pub unresolved: usize,
}

/// A line's state, as the thread leaves it.
const UNTRIED: u8 = 0;
const TRYING: u8 = 1;
const WRITTEN: u8 = 2;
const FAILED: u8 = 3;

/// Lines on descriptors that could not be duplicated (a full descriptor
/// table, 2.11's *The main thread's `exit` line*), in order, on a thread of
/// their own waited on until `until`: no write blocks the caller past it,
/// whatever another writer of the same pipe does or the host's writeback
/// takes. The thread tries every line, one that fails leaving the next its
/// own try, so a failing stdout never costs stderr its line; it checks
/// `until` before every try and writes a non-regular file in pieces of at
/// most `PIPE_BUF`, each after `poll` finds it writable
/// (`send(MSG_DONTWAIT)` on a socket); one still blocked when the wait
/// ends is left to the process's exit, its piece whole. Gives each line's
/// [`Outcome`], read once the wait ends: the thread marks a line tried
/// before its try and settled after it, so a line whose write returned
/// but was not yet marked is unresolved, never failed. The caller's
/// description and flags are left as they are.
///
/// The descriptors must stay open for the process's life: the callers pass
/// only stdout and stderr, which the engine never closes.
pub(crate) fn write(
    lines: &[(std::os::fd::BorrowedFd<'_>, &[u8])],
    until: std::time::Instant,
    on_start: Option<fn()>,
) -> Outcome {
    // Ends with the thread's every line settled, or no sooner than
    // `until`: a line not yet tried then gives up at its first check.
    write_with(lines, until, on_start, raw_line, |settled| {
        let _ = settled.recv_timeout(until.saturating_duration_since(std::time::Instant::now()));
    })
}

/// [`write`], each line written by `raw` ([`raw_line`]'s contract) and the
/// wait for the thread's lines to settle done by `wait`, given the channel
/// the thread sends on once they all are: a test's seams, so a test ends
/// the wait when its held write is known to be in its try.
fn write_with(
    lines: &[(std::os::fd::BorrowedFd<'_>, &[u8])],
    until: std::time::Instant,
    on_start: Option<fn()>,
    raw: impl Fn(i32, &[u8], std::time::Instant) -> bool + Send + 'static,
    wait: impl FnOnce(&std::sync::mpsc::Receiver<()>),
) -> Outcome {
    use std::os::fd::AsRawFd;
    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering::SeqCst},
    };
    let lines: Vec<(i32, Vec<u8>)> = lines
        .iter()
        .filter(|(_, l)| !l.is_empty())
        .map(|(fd, l)| (fd.as_raw_fd(), l.to_vec()))
        .collect();
    let total = lines.len();
    if total == 0 {
        return Outcome::default();
    }
    // Each line's state, read once the wait ends.
    let states: Arc<[AtomicU8]> = lines.iter().map(|_| AtomicU8::new(UNTRIED)).collect();
    let marks = states.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    let spawned = std::thread::Builder::new()
        .name("exit-writer".into())
        .stack_size(64 << 10)
        .spawn(move || {
            if let Some(register) = on_start {
                register();
            }
            for (state, (fd, line)) in marks.iter().zip(lines) {
                state.store(TRYING, SeqCst);
                let whole = raw(fd, &line, until);
                state.store(if whole { WRITTEN } else { FAILED }, SeqCst);
            }
            let _ = tx.send(());
        });
    if spawned.is_err() {
        return Outcome {
            failed: total,
            ..Outcome::default()
        };
    }
    wait(&rx);
    let mut outcome = Outcome::default();
    for state in states.iter() {
        match state.load(SeqCst) {
            WRITTEN => outcome.written += 1,
            TRYING => outcome.unresolved += 1,
            _ => outcome.failed += 1,
        }
    }
    outcome
}

/// One line on the raw descriptor `fd` ([`write`]'s thread): gives
/// whether it was written whole by `until`.
fn raw_line(fd: i32, line: &[u8], until: std::time::Instant) -> bool {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is stdout or stderr, open for the process's life
    // ([`write`]); stat has room for fstat's output.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: a successful fstat initialized it.
    let kind = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
    let mut rest = line;
    while !rest.is_empty() {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        let piece = rest
            .get(..rest.len().min(libc::PIPE_BUF))
            .unwrap_or_default();
        if kind != libc::S_IFREG && kind != libc::S_IFBLK {
            let ms = i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX);
            let mut poll = [libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            }];
            // SAFETY: one initialized pollfd.
            let ready = unsafe { libc::poll(poll.as_mut_ptr(), 1, ms) };
            if ready != 1 || poll[0].revents & libc::POLLOUT == 0 {
                if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
            if std::time::Instant::now() >= until {
                return false;
            }
        }
        // SAFETY: piece is readable for its length; `fd` as above.
        let n = unsafe {
            if kind == libc::S_IFSOCK {
                libc::send(fd, piece.as_ptr().cast(), piece.len(), libc::MSG_DONTWAIT)
            } else {
                libc::write(fd, piece.as_ptr().cast(), piece.len())
            }
        };
        match usize::try_from(n) {
            Ok(n) if n > 0 => rest = rest.get(n..).unwrap_or_default(),
            _ if matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
#[path = "borrowed_tests.rs"]
mod tests;
