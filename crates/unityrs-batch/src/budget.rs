//! Sealed, nonblocking attempt accounts (`batch.md` 2.2).
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use unityrs_core::budget::{
    AllocError, BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation,
};

#[derive(Debug)]
struct Account {
    limit: u64,
    used: AtomicU64,
    peak: AtomicU64,
    refusals: AtomicU64,
    telemetry: std::sync::OnceLock<Arc<AtomicU64>>,
}
impl Account {
    fn new(limit: u64, telemetry: Option<Arc<AtomicU64>>) -> Arc<Self> {
        let observed = std::sync::OnceLock::new();
        if let Some(telemetry) = telemetry {
            let _ = observed.set(telemetry);
        }
        Arc::new(Self {
            limit,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            refusals: AtomicU64::new(0),
            telemetry: observed,
        })
    }
    fn snapshot(&self) -> AccountSnapshot {
        AccountSnapshot {
            used: self.used.load(Ordering::Acquire),
            peak: self.peak.load(Ordering::Acquire),
            refusals: self.refusals.load(Ordering::Acquire),
        }
    }
}
impl PermitHome for Account {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        if let Some(used) = self.telemetry.get() {
            used.fetch_sub(bytes, Ordering::AcqRel);
        }
    }
}

/// One stable permit home per reservation, never shared between attempts.
/// Provider is always refused, including a request for zero bytes.
#[derive(Debug)]
pub struct SealedBudget {
    input: Arc<Account>,
    dependency: Arc<Account>,
    input_only: bool,
    provider_refusals: AtomicU64,
    allocator_refusals: AtomicU64,
}
impl SealedBudget {
    /// Small caps are allowed for library tests; spec validation owns floors.
    #[must_use]
    pub fn new(m: u64, d: u64) -> Self {
        Self::observed(m, d, false, None)
    }
    #[must_use]
    pub fn observed(m: u64, d: u64, input_only: bool, telemetry: Option<Arc<AtomicU64>>) -> Self {
        Self {
            input: Account::new(m, telemetry.clone()),
            dependency: Account::new(d, telemetry),
            input_only,
            provider_refusals: AtomicU64::new(0),
            allocator_refusals: AtomicU64::new(0),
        }
    }
    /// A standalone peek: Dependency and Provider are refused even for zero
    /// bytes. An ordinary attempt with `d == 0` keeps its existing semantics.
    #[must_use]
    pub fn input_only(m: u64) -> Self {
        Self {
            input_only: true,
            ..Self::new(m, 0)
        }
    }
    pub(crate) fn observe(&self, usage: Arc<AtomicU64>) {
        let _ = self.input.telemetry.set(usage.clone());
        let _ = self.dependency.telemetry.set(usage);
    }
    /// Read the sticky allocator audit without sampling either account.
    pub(crate) fn allocator_refusals(&self) -> u64 {
        self.allocator_refusals.load(Ordering::Acquire)
    }
    /// Sample after all attempt locals drop for the leak check. During
    /// concurrent reservations this is observational, not a coherent census.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            input: self.input.snapshot(),
            dependency: self.dependency.snapshot(),
            provider_refusals: self.provider_refusals.load(Ordering::Acquire),
            allocator_refusals: self.allocator_refusals.load(Ordering::Acquire),
        }
    }
}
fn refused(n: &AtomicU64) {
    let _ = n.try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_add(1))
    });
}
impl MemoryBudget for SealedBudget {
    fn allocator_refused(&self, _error: AllocError) {
        refused(&self.allocator_refusals);
    }

    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let account = match what {
            Reservation::Input => &self.input,
            Reservation::Dependency => {
                if self.input_only {
                    refused(&self.dependency.refusals);
                    return Err(BudgetExceeded {
                        requested: bytes,
                        what,
                    });
                }
                &self.dependency
            }
            Reservation::Provider => {
                refused(&self.provider_refusals);
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
        };
        let result = account
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&n| n <= account.limit)
            });
        if let Ok(old) = result {
            account
                .peak
                .fetch_max(old.saturating_add(bytes), Ordering::AcqRel);
            if let Some(used) = account.telemetry.get() {
                used.fetch_add(bytes, Ordering::AcqRel);
            }
            Ok(Permit::new(bytes, account.clone()))
        } else {
            refused(&account.refusals);
            Err(BudgetExceeded {
                requested: bytes,
                what,
            })
        }
    }
}

/// Figures for one reservation; peaks and refusals survive releases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountSnapshot {
    /// Live charged bytes.
    pub used: u64,
    /// Greatest live byte count.
    pub peak: u64,
    /// Refused calls, saturating only at the integer maximum.
    pub refusals: u64,
}
/// Fixed-size figures; no account permits or extracted data escape here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Input's account.
    pub input: AccountSnapshot,
    /// Private Environment's account.
    pub dependency: AccountSnapshot,
    /// Unexpected provider reservations.
    pub provider_refusals: u64,
    /// Host allocator refusals, independent of budget reservations. Sticky
    /// for the attempt's lifetime, including when optional work absorbed one.
    pub allocator_refusals: u64,
}
impl Snapshot {
    /// Both accounts are empty; a false result makes the slot unreleasable.
    #[must_use]
    pub fn is_clear(self) -> bool {
        self.input.used == 0 && self.dependency.used == 0
    }
    /// Refusals used by the export seam's probes.
    #[must_use]
    pub fn refusals(self) -> u64 {
        self.input
            .refusals
            .saturating_add(self.dependency.refusals)
            .saturating_add(self.provider_refusals)
    }
}

#[cfg(test)]
mod allocator_tests {
    use super::*;
    #[test]
    fn allocator_audit_is_independent_and_survives_charge_release() {
        let budget = Arc::new(SealedBudget::new(10, 10));
        let meter =
            unityrs_core::limits::Meter::new(u64::MAX, budget.clone(), Reservation::Dependency);
        let charge = meter.charge(10).unwrap();
        let _ = meter.allocator_refused(4096);
        assert_eq!(budget.snapshot().refusals(), 0);
        assert_eq!(budget.snapshot().allocator_refusals, 1);
        assert!(meter.charge(1).is_err());
        drop(charge);
        let snapshot = budget.snapshot();
        assert!(snapshot.is_clear());
        assert_eq!(snapshot.allocator_refusals, 1);
        assert_eq!(snapshot.refusals(), 1);
    }
}
