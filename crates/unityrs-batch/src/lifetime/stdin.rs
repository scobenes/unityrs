//! The stdin watch (`stop_on_stdin_eof`), on a small thread of its own that
//! calls [`Process::request`], so no stdin read can ever hold the lifecycle
//! thread's hard exit: that thread polls only its self-pipe and timer.
//!
//! Caller duty that remains: prepare this before any service thread, spawn
//! [`StdinWatch::run`] on its own dedicated thread, and never join that
//! thread. Its `poll` waits for readiness, and its read takes at most
//! `FIONREAD`'s count, but another reader of the same open description (a
//! pager on the same terminal, say) can still take those bytes first, and the
//! read then blocks. Only this thread waits; the process ends by `_exit`,
//! which ends it too. fd 0 is never reopened, closed or re-flagged, and no
//! other in-process code reads it. All storage is prepared before the spawn,
//! so the thread allocates nothing.
use super::{Process, Stop};
use crate::{
    diagnostic::{LINE_BYTES, Secrets, Writer},
    reason::StopReason,
    sys::lifecycle::{self, Input, Stdin},
};
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
};
pub struct StdinWatch<'a> {
    process: &'a Process,
    warning: &'a AtomicBool,
    diagnostic: &'a Writer,
    secrets: Secrets<'a>,
    stdin: Stdin,
    input: [u8; 4096],
    line: [u8; LINE_BYTES],
}
impl<'a> StdinWatch<'a> {
    /// Checks fd 0 (a pipe, FIFO, socket or terminal, open for reading).
    pub fn prepare(
        process: &'a Process,
        warning: &'a AtomicBool,
        diagnostic: &'a Writer,
        secrets: Secrets<'a>,
    ) -> io::Result<Self> {
        Ok(Self {
            process,
            warning,
            diagnostic,
            secrets,
            stdin: Stdin::prepare()?,
            input: [0; 4096],
            line: [0; LINE_BYTES],
        })
    }
    /// Returns once the watch has ended: at EOF, after requesting the
    /// `stdin_eof` stop; or, ended without EOF, after storing the warning and
    /// writing its one stderr line, with no stop.
    pub fn run(mut self) {
        loop {
            let mut fds = [libc::pollfd {
                fd: self.stdin.fd(),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            }];
            if lifecycle::poll(&mut fds, -1).is_err() {
                self.process.request(Stop::external(StopReason::Internal));
                return;
            }
            let [fd] = fds;
            if fd.revents == 0 {
                continue;
            }
            match self.stdin.read(fd.revents, &mut self.input) {
                Input::Again => {}
                Input::Eof => {
                    self.process.request(Stop::external(StopReason::StdinEof));
                    return;
                }
                Input::Ended => {
                    self.warning.store(true, SeqCst);
                    self.diagnostic.line(
                        "stdin watch ended\n",
                        &self.secrets,
                        &mut self.line,
                        true,
                    );
                    return;
                }
            }
        }
    }
}
