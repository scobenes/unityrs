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
    io,
    os::fd::AsFd,
    os::fd::OwnedFd,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
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
    note: Option<Note>,
    noted: AtomicBool,
}
pub(super) struct Note {
    pub bytes: [u8; crate::diagnostic::LINE_BYTES],
    pub len: usize,
}
impl Early {
    pub fn new(
        clock: Clock,
        deadline: Option<i128>,
        spare: [Option<OwnedFd>; 2],
        note: Option<&str>,
    ) -> Self {
        let note = note.map(|text| {
            let mut bytes = [0; crate::diagnostic::LINE_BYTES];
            let mut len = text.len().min(bytes.len());
            while !text.is_char_boundary(len) {
                len -= 1;
            }
            if let Some(target) = bytes.get_mut(..len) {
                target.copy_from_slice(text.as_bytes().get(..len).unwrap_or_default());
            }
            Note { bytes, len }
        });
        Self {
            clock,
            deadline,
            boot: AtomicU64::new(u64::MAX),
            spare: Mutex::new(spare),
            note,
            noted: AtomicBool::new(false),
        }
    }
    pub fn take_note(&self) -> Option<&Note> {
        if self.noted.swap(true, SeqCst) {
            None
        } else {
            self.note.as_ref()
        }
    }
    pub fn has_note(&self) -> bool {
        self.note.is_some() && !self.noted.load(SeqCst)
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
    /// The successful warning retains the early writer's deadline budget.
    pub fn note_wait(&self) -> Duration {
        match self.budget() {
            Wait::Own => progress::early::OWN_WAIT,
            Wait::Within(wait) => wait,
            Wait::Never => Duration::ZERO,
        }
    }
    /// Try the warning before stdout, with its own half-budget deadline.
    /// The remaining diagnostics retain the original total output bound.
    pub fn lines(&self, out: &[u8], note: &[u8], err: &[u8]) {
        self.release();
        let stdout = io::stdout();
        let stderr = io::stderr();
        let budget = self.budget();
        let wait = match budget {
            Wait::Own => progress::early::OWN_WAIT,
            Wait::Within(wait) => wait,
            Wait::Never => Duration::ZERO,
        };
        let started = Instant::now();
        let until = started + wait;
        if !note.is_empty() {
            let note_until = started + wait / 2;
            match budget {
                Wait::Own => progress::early::write_on_until([(stderr.as_fd(), note)], note_until),
                Wait::Within(_) => {
                    let _ = progress::early::within_on(
                        [(stderr.as_fd(), note)],
                        note_until.saturating_duration_since(Instant::now()),
                    );
                }
                Wait::Never => {}
            }
        }
        let lines = [(stdout.as_fd(), out), (stderr.as_fd(), err)];
        match budget {
            Wait::Own => progress::early::write_on_until(lines, until),
            Wait::Within(_) => {
                let _ = progress::early::within_on(
                    lines,
                    until.saturating_duration_since(Instant::now()),
                );
            }
            Wait::Never => {}
        }
        // The reservation kept for a later 69 when this one ends nothing.
        self.restore();
    }
    /// A terminal startup failure after the progress service exists. With
    /// no controller, a bounded early writer substitutes for its stack.
    pub fn terminal_note(&self, secrets: &crate::diagnostic::Secrets<'_>) {
        if let Some(note) = self.take_note() {
            let text = std::str::from_utf8(note.bytes.get(..note.len).unwrap_or_default())
                .unwrap_or_default();
            let mut buffer = [0; crate::diagnostic::LINE_BYTES];
            let len = secrets.mask(text, &mut buffer);
            self.lines(b"", buffer.get(..len).unwrap_or_default(), b"");
        }
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
        // A failure before the progress service tries the pending warning
        // first, within the terminal diagnostics' total output bound.
        let mut buffer = [0; crate::diagnostic::LINE_BYTES];
        let note = if let Some(note) = self.take_note() {
            let text = std::str::from_utf8(note.bytes.get(..note.len).unwrap_or_default())
                .unwrap_or_default();
            match secrets {
                Some(secrets) => {
                    let len = secrets.mask(text, &mut buffer);
                    buffer.get(..len).unwrap_or_default()
                }
                None => text.as_bytes(),
            }
        } else {
            &[]
        };
        self.lines(snapshot.line(Some(&exit)).as_bytes(), note, err);
    }
    /// A 69 before the process word exists: nothing to reset.
    pub fn fail(&self, error: &Error) -> ! {
        self.write(error);
        crate::sys::lifecycle::exit_unprepared(error.code)
    }
}
