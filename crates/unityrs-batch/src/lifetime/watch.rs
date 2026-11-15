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
    /// After a panic's line is queued, before the exit: the host's write of
    /// the queued lines.
    on_panic: fn(),
    #[cfg(feature = "test-hooks")]
    trace: Option<std::fs::File>,
    #[cfg(feature = "test-hooks")]
    cutoff_arm: Option<(std::fs::File, u64)>,
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
            on_panic: || {},
            #[cfg(feature = "test-hooks")]
            trace: None,
            #[cfg(feature = "test-hooks")]
            cutoff_arm: None,
            #[cfg(feature = "test-hooks")]
            mark: || {},
        })
    }
    /// What a panic of the thread calls before its exit.
    pub fn on_panic(&mut self, on_panic: fn()) {
        self.on_panic = on_panic;
    }
    /// The lifecycle trace FIFO step 1 opened
    /// ([`crate::sys::lifecycle::open_trace`]): never a regular file.
    #[cfg(feature = "test-hooks")]
    pub fn with_trace(&mut self, file: std::fs::File) {
        self.trace = Some(file);
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
    pub fn cutoff_after(&mut self, fifo: std::fs::File, delay_ms: u64) {
        self.cutoff_arm = Some((fifo, delay_ms));
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
        if result.is_err() {
            crate::diagnostic::pending::panic(format_args!("the lifecycle thread: "));
            (self.on_panic)();
        }
        // The more severe of 70 and the code so far: a stored 69 stays.
        self.process
            .exit(Code::Internal.worst(self.process.state.end_code(None)))
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
        // No timer on this host (off Linux): bounded polls from the start,
        // with no stop, since nothing failed.
        let mut fallback = self.timer.absent();
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
            #[cfg_attr(feature = "test-hooks", allow(unused_mut))]
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
            #[cfg(feature = "test-hooks")]
            let mut fds = [
                fds[0],
                fds[1],
                libc::pollfd {
                    fd: self
                        .cutoff_arm
                        .as_ref()
                        .map_or(-1, |(file, _)| file.as_raw_fd()),
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
            #[cfg(feature = "test-hooks")]
            if let Some((file, delay)) = &mut self.cutoff_arm {
                use std::io::Read;
                if file.read(&mut [0]).is_ok_and(|n| n == 1) {
                    let Ok(now) = Boot::now() else {
                        self.process.exit(Code::Internal);
                    };
                    self.policy.cutoff = Some(Boot(now.0.saturating_add(*delay)));
                    self.cutoff_arm = None;
                    self.traced(b"cutoff armed\n");
                }
            }
        }
    }
}
