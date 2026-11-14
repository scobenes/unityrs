//! W9: lifecycle owns the hard exit, never an engine mutex or a Tokio wake.
//! Storage is constructed before start. run makes no heap allocation, channel
//! send, unpark, cancellation call or regular-file diagnostic after entry.
//! Its poll set is its own self-pipe and timer alone, both nonblocking: it
//! never reads stdin, whose watch runs on its own thread ([`super::StdinWatch`]),
//! so no read can hold the hard exit.
use super::{Boot, Policy, Process, Stop};
use crate::{
    reason::{Code, StopReason},
    sys::{
        Wake,
        lifecycle::{self, Timer},
    },
};
use std::{io, os::fd::AsRawFd};
pub struct Watch<'a> {
    pub process: &'a Process,
    pub policy: Policy,
    pub controller: &'a Wake,
    pub finisher: &'a Wake,
    timer: Timer,
    #[cfg(feature = "test-hooks")]
    trace: Option<std::fs::File>,
    #[cfg(feature = "test-hooks")]
    mark: fn(),
}
impl<'a> Watch<'a> {
    pub fn prepare(
        process: &'a Process,
        policy: Policy,
        controller: &'a Wake,
        finisher: &'a Wake,
    ) -> io::Result<Self> {
        Ok(Self {
            process,
            policy,
            controller,
            finisher,
            timer: Timer::new()?,
            #[cfg(feature = "test-hooks")]
            trace: None,
            #[cfg(feature = "test-hooks")]
            mark: || {},
        })
    }
    /// A lifecycle trace can only be a fresh nonblocking FIFO, never a file.
    #[cfg(feature = "test-hooks")]
    pub fn trace(&mut self, path: &std::path::Path) -> io::Result<()> {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
            .open(path)?;
        if !file.metadata()?.file_type().is_fifo() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.trace = Some(file);
        Ok(())
    }
    // Without `test-hooks` the trace is compiled out, and with it `self`.
    #[cfg_attr(not(feature = "test-hooks"), allow(clippy::unused_self))]
    fn traced(&self, line: &[u8]) {
        #[cfg(feature = "test-hooks")]
        if let Some(file) = &self.trace {
            crate::sys::lifecycle::trace(file, line);
        }
        #[cfg(not(feature = "test-hooks"))]
        let _ = line;
    }
    #[cfg(feature = "test-hooks")]
    pub fn fail_timer(&mut self) {
        self.timer.fail_arm();
    }
    #[cfg(feature = "test-hooks")]
    pub fn mark(&mut self, mark: fn()) {
        self.mark = mark;
    }
    pub fn run(mut self) -> ! {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.drive()));
        let _ = result;
        self.process.exit(Code::Internal)
    }
    #[allow(clippy::too_many_lines)] // The ordered W9 state machine stays in one audit scope.
    fn drive(&mut self) -> ! {
        #[cfg(feature = "test-hooks")]
        (self.mark)();
        // A stop may already be pending when the lifecycle thread starts.
        // Observe that external byte before doing its wake work as well.
        if self.process.pipe.drain() {
            self.traced(b"lifecycle_pipe_read reached\n");
        }
        let mut notified = false;
        let mut cutoff_seen = false;
        let mut programmed = None;
        let mut fallback = false;
        loop {
            let Ok(now) = Boot::now() else {
                self.process.exit(Code::Internal)
            };
            if !cutoff_seen && self.policy.cutoff.is_some_and(|cut| now >= cut) {
                cutoff_seen = true;
                self.process.request(Stop::external(StopReason::Deadline));
            }
            if self.process.state.live().is_some() && !notified {
                let Ok(hard) = self.policy.hard(now) else {
                    self.process.exit(Code::Internal)
                };
                self.process.state.arm(hard);
                if self.controller.notify().is_err() {
                    self.process.request(Stop::external(StopReason::Internal));
                }
                notified = true;
            }
            let hard = self.process.state.hard();
            if hard.is_some_and(|hard| now >= hard) {
                self.process.exit(self.process.state.end_code(None));
            }
            let next = hard.or(self.policy.cutoff);
            if next != programmed && !fallback {
                if let Some(next) = next
                    && self.timer.arm(next).is_err()
                {
                    self.process.request(Stop::external(StopReason::Internal));
                    fallback = true;
                    continue;
                }
                programmed = next;
                if notified {
                    if self.finisher.notify().is_err() {
                        self.process.request(Stop::external(StopReason::Internal));
                    }
                    self.traced(b"lifecycle_stop_woken reached\n");
                }
            } else if notified && fallback && programmed != hard {
                programmed = hard;
                let _ = self.finisher.notify();
                self.traced(b"lifecycle_stop_woken reached\n");
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.process.pipe.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.timer.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let timeout = if fallback {
                i32::try_from(next.map_or(1000, |n| n.0.saturating_sub(now.0).min(1000)))
                    .unwrap_or(1000)
            } else {
                -1
            };
            if lifecycle::poll(&mut fds, timeout).is_err() {
                self.process.request(Stop::external(StopReason::Internal));
            }
            // Every return, including EINTR, goes around the boot-clock check.
            if self.process.pipe.drain() {
                self.traced(b"lifecycle_pipe_read reached\n");
            }
            self.timer.drain();
        }
    }
}
