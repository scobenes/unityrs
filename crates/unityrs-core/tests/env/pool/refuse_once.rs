use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};

/// A budget that grants every reservation but one: the `n`-th dependency
/// reservation after it is armed with `n` ([`Self::arm`]), refused once.
/// It counts the dependency reservations it is asked for.
#[derive(Debug, Default)]
pub(super) struct RefuseOnce {
    countdown: AtomicU64,
    asked: AtomicU64,
    refused: AtomicU64,
}

impl RefuseOnce {
    pub(super) fn arm(&self, n: u64) {
        self.countdown.store(n, Ordering::Release);
    }

    pub(super) fn asked(&self) -> u64 {
        self.asked.load(Ordering::Acquire)
    }

    pub(super) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Acquire)
    }
}

impl MemoryBudget for RefuseOnce {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Dependency {
            self.asked.fetch_add(1, Ordering::AcqRel);
            let left = self
                .countdown
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
            if left == Ok(1) {
                self.refused.fetch_add(1, Ordering::AcqRel);
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
        }
        Ok(Permit::detached(bytes))
    }
}
