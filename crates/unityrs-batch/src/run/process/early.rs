//! The main thread's own lines (2.11's *The main thread's `exit` line*):
//! before the progress thread exists, the main thread writes its `exit`
//! line and stderr line itself, through the writer thread with a deadline.
//! Two spare descriptors reserved at step 1 are closed first, so a 69 of a
//! full descriptor table (`EMFILE`) still has the room its writes take.
use super::Error;
use crate::{
    lifetime::Boot,
    progress::{self, Exit, Phase, Snapshot, clock::Clock},
};
use std::{
    os::fd::OwnedFd,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering::SeqCst},
    },
    time::Duration,
};

/// Step 1's two spare descriptors (`/dev/null`; std opens `O_CLOEXEC`).
pub(super) fn spares() -> [Option<OwnedFd>; 2] {
    let open = || std::fs::File::open("/dev/null").ok().map(OwnedFd::from);
    [open(), open()]
}
/// How the main thread writes: itself (no deadline), through its writer
/// thread for at most the time given, or not at all (no time left).
enum Wait {
    Own,
    Within(Duration),
    Never,
}
pub(super) struct Early {
    pub clock: Clock,
    /// The deadline in Unix nanoseconds, read by the wall clock.
    pub deadline: Option<i128>,
    /// The deadline on `CLOCK_BOOTTIME` once step 6 converted it
    /// (milliseconds; `u64::MAX` before).
    boot: AtomicU64,
    spare: Mutex<[Option<OwnedFd>; 2]>,
}
impl Early {
    pub fn new(clock: Clock, deadline: Option<i128>, spare: [Option<OwnedFd>; 2]) -> Self {
        Self {
            clock,
            deadline,
            boot: AtomicU64::new(u64::MAX),
            spare: Mutex::new(spare),
        }
    }
    /// Step 6's conversion: the time left is read on the boot clock after.
    pub fn converted(&self, deadline: Option<Boot>) {
        if let Some(deadline) = deadline {
            self.boot.store(deadline.0, SeqCst);
        }
    }
    /// The spares closed: before an early write, or once the progress
    /// thread writes the lines instead.
    pub fn release(&self) {
        let mut spare = self.spare.lock().unwrap_or_else(PoisonError::into_inner);
        *spare = [None, None];
    }
    /// The spares opened again after a write that does not end the run
    /// (the uncertified page size's note).
    fn restore(&self) {
        let mut spare = self.spare.lock().unwrap_or_else(PoisonError::into_inner);
        if spare.iter().all(Option::is_none) {
            *spare = spares();
        }
    }
    /// With a deadline, how long the writer thread may be waited on:
    /// `min(1 s, the time left - 500 ms)`, `None` when not positive; by the
    /// wall clock before step 6's conversion, `CLOCK_BOOTTIME` after.
    fn budget(&self) -> Wait {
        let Some(deadline) = self.deadline else {
            return Wait::Own;
        };
        let boot = self.boot.load(SeqCst);
        let left_ms = if boot == u64::MAX {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX));
            deadline.saturating_sub(now) / 1_000_000
        } else {
            let now = Boot::now().map_or(u64::MAX, |b| b.0);
            i128::from(boot) - i128::from(now)
        };
        match u64::try_from(left_ms.saturating_sub(500)) {
            Ok(ms) if ms > 0 => {
                Wait::Within(Duration::from_millis(ms).min(progress::early::OWN_WAIT))
            }
            _ => Wait::Never,
        }
    }
    /// `out` on stdout, then `err` on stderr, as the main thread writes
    /// them: itself without a deadline, its writer thread with one.
    pub fn lines(&self, out: &[u8], err: &[u8]) {
        self.release();
        match self.budget() {
            Wait::Own => progress::early::write(out, err),
            Wait::Within(wait) => {
                let _ = progress::early::within(out, err, wait);
            }
            Wait::Never => {}
        }
        // The reservation kept for a later 69 when this one ends nothing.
        self.restore();
    }
    /// A post-validation warning, using the already prepared spec masker.
    pub fn note(&self, note: &str) {
        let mut buffer = [0; crate::diagnostic::LINE_BYTES];
        let n = super::current_secrets().mask(note, &mut buffer);
        self.lines(b"", buffer.get(..n).unwrap_or_default());
    }
    /// A start's line, before `running`: `seq` 0, its stderr line
    /// `unityrs batch: <reason> <detail>`.
    pub fn write(&self, error: &Error) {
        self.write_masked(error, Some(&super::current_secrets()));
    }
    /// The host's early stderr route, using the spec-only or resolved
    /// secret slice. Structured progress details retain their typed names.
    pub fn write_masked(&self, error: &Error, secrets: Option<&crate::diagnostic::Secrets<'_>>) {
        let mut snapshot = Snapshot::validate(self.clock.ms());
        snapshot.phase = Phase::Preflight;
        snapshot.memory.rss = progress::rss();
        let err = match &error.detail {
            Some(detail) => format!("unityrs batch: {} {detail}\n", error.reason),
            None => format!("unityrs batch: {}\n", error.reason),
        };
        let exit = Exit {
            status: None,
            code: error.code,
            reason: Some(error.reason),
            detail: error.detail.clone().map(Into::into),
        };
        let mut masked = [0; crate::diagnostic::LINE_BYTES];
        let err = match secrets {
            Some(secrets) => {
                let len = secrets.mask(&err, &mut masked);
                masked.get(..len).unwrap_or_default()
            }
            None => err.as_bytes(),
        };
        self.lines(snapshot.line(Some(&exit)).as_bytes(), err);
    }
    /// A 69 before the process word exists: nothing to reset.
    pub fn fail(&self, error: &Error) -> ! {
        self.write(error);
        crate::sys::lifecycle::exit_unprepared(error.code)
    }
}
