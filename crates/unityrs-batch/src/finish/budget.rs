//! The finisher's try budget (`docs/plans/batch.md` 2.11, steps 1 to 3) and
//! its backend seams. Once `T_hard` is armed, every try but the head's ends
//! by `T_hard - head_reserve`, so a row file is cut before it can take the
//! head's reserve; unarmed, nothing is cut. The journal seam is B11's (an
//! object store's uploads journal); the directory sink has no journal.
use crate::{
    lifetime::{Boot, Policy},
    manifest::{Family, WriteError, chunks::Destination},
};
use std::time::Duration;

/// The finisher's clock: `CLOCK_BOOTTIME` in production, scripted in tests.
pub trait Clock {
    /// Now; an unreadable clock reads as past every deadline, so the
    /// finisher cuts what it may cut and still tries the head.
    fn now(&self) -> Boot;
    fn sleep(&self, ms: u64);
}
pub struct BootClock;
impl Clock for BootClock {
    fn now(&self) -> Boot {
        Boot::now().unwrap_or(Boot(u64::MAX - 1))
    }
    fn sleep(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// `T_hard`, once armed, and whether 7.4's second row (a `running` write's
/// reconciliation) keeps 5 s for the head instead of 3 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub hard: Option<Boot>,
    pub reconcile: bool,
}
impl Budget {
    pub const UNARMED: Self = Self {
        hard: None,
        reconcile: false,
    };
    /// Where every try but the head's ends: `T_hard - head_reserve`.
    #[must_use]
    pub fn row_deadline(&self) -> Option<Boot> {
        self.hard.map(|h| Policy::non_head_cut(h, self.reconcile))
    }
    /// A row-file chunk starts only with the head's reserve still ahead.
    #[must_use]
    pub fn rows_open(&self, now: Boot) -> bool {
        self.row_deadline().is_none_or(|d| now < d)
    }
    /// The head's own requests end by `T_hard - 2 s`.
    #[must_use]
    pub fn head_deadline(&self) -> Option<Boot> {
        self.hard.map(Policy::head_cut)
    }
    /// Step 2's per-abort timeout: `min(5 s, T_hard - now - head_reserve)`,
    /// 5 s unarmed, none under 500 ms.
    #[must_use]
    pub fn abort_timeout(&self, now: Boot) -> Option<Duration> {
        match self.hard {
            None => Some(Duration::from_secs(5)),
            Some(hard) => Policy::abort_ms(hard, now, self.reconcile).map(Duration::from_millis),
        }
    }
}

/// Step 1's poll of the journal's mutex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// Marked closed under the mutex: nothing removes a line after.
    Closed,
    /// The mutex is held (a throttled append, W7): poll again in 10 ms.
    Busy,
    /// No journal: the spool's lock was never taken, or a directory sink.
    Absent,
}
/// The uploads journal and its aborts (B11 ruling G9). Only the directory
/// backend exists in version 1's engine so far; it has no journal.
pub trait Backend {
    fn try_close(&mut self) -> Close;
    /// The next upload id the closed journal holds, by index.
    fn next_upload(&mut self) -> Option<u64>;
    /// Abort one upload within `timeout`; false when it failed.
    fn abort(&mut self, upload: u64, timeout: Duration) -> bool;
}
pub struct NoJournal;
impl Backend for NoJournal {
    fn try_close(&mut self) -> Close {
        Close::Absent
    }
    fn next_upload(&mut self) -> Option<u64> {
        None
    }
    fn abort(&mut self, _: u64, _: Duration) -> bool {
        true
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    pub closed: bool,
    pub aborted: u64,
    /// The head's `abort_failed` warning.
    pub abort_failed: u64,
    /// Left to the next start's aborts (`BATCH_SPEC.md` 6.2).
    pub left: u64,
}
/// Steps 1 and 2: close the journal, polling every 10 ms and giving up at
/// `T_hard - head_reserve` once armed, then abort what it holds while each
/// abort still has 500 ms. Giving up skips both steps.
pub fn settle(backend: &mut dyn Backend, budget: Budget, clock: &dyn Clock) -> Settled {
    let mut out = Settled::default();
    loop {
        match backend.try_close() {
            Close::Closed => break,
            Close::Absent => return out,
            Close::Busy => {
                if budget.row_deadline().is_some_and(|d| clock.now() >= d) {
                    return out;
                }
                clock.sleep(10);
            }
        }
    }
    out.closed = true;
    while let Some(upload) = backend.next_upload() {
        match budget.abort_timeout(clock.now()) {
            Some(timeout) if backend.abort(upload, timeout) => out.aborted += 1,
            Some(_) => out.abort_failed += 1,
            None => out.left += 1,
        }
    }
    out
}

/// A row-file store whose every try carries its deadline.
pub trait Rows {
    fn put(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: &[u8],
        deadline: Option<Boot>,
    ) -> Result<(), WriteError>;
}
impl<D: Destination> Rows for D {
    /// A directory write cannot be ended early; its start is what the
    /// budget bounds, and `_exit` at `T_hard` ends a held one.
    fn put(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: &[u8],
        _: Option<Boot>,
    ) -> Result<(), WriteError> {
        self.publish(family, chunk, bytes)
    }
}
/// Step 3's row files under the budget: a chunk that would start inside
/// the head's reserve is skipped, and with it the rest of its family,
/// which the head then names null (a 4).
pub struct Budgeted<'a> {
    pub rows: &'a mut dyn Rows,
    pub budget: Budget,
    pub clock: &'a dyn Clock,
    pub cut: bool,
}
impl Destination for Budgeted<'_> {
    fn publish(&mut self, family: Family, chunk: u64, bytes: &[u8]) -> Result<(), WriteError> {
        if !self.budget.rows_open(self.clock.now()) {
            self.cut = true;
            return Err(WriteError::Publish);
        }
        self.rows
            .put(family, chunk, bytes, self.budget.row_deadline())
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
