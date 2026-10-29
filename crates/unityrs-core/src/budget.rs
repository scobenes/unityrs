//! Memory budgets: the mechanism in the core, the policy in the batch engine.
//!
//! Before core allocates a buffer whose size the input chose (a decoded block
//! run, an inflated stream), it asks the file's [`MemoryBudget`] for a
//! [`Permit`]. The request never blocks: blocking inside a rayon pool can
//! deadlock, so the batch engine does its blocking admission *before* it hands
//! work to the pool, and gives core a prepaid budget (plan section 4.1).
//!
//! A permit rides inside the buffer it paid for ([`budgeted_bytes`]), so the
//! bytes are returned only when the last `Bytes` slice of that buffer drops,
//! whoever holds it. The books therefore always show what is alive.
//!
//! Owner: the scaffold (implemented); package C maintains it.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

/// What a reservation pays for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reservation {
    /// A bundle's decoded block run.
    BundleData,
    /// An inflated gzip or brotli stream.
    Inflate,
    /// A file loaded only as a dependency (the environment's LRU).
    Dependency,
}

/// A refused reservation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("memory budget refused {requested} bytes for {what:?}")]
pub struct BudgetExceeded {
    /// Bytes asked for.
    pub requested: u64,
    /// What they were for.
    pub what: Reservation,
}

/// A source of permits. Implementations must not block.
pub trait MemoryBudget: Send + Sync + fmt::Debug {
    /// Reserves `bytes`, or refuses at once.
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded>;
}

/// Where a permit returns its bytes.
pub trait PermitHome: Send + Sync {
    /// Takes `bytes` back.
    fn release(&self, bytes: u64);
}

/// Bytes reserved from a budget, returned when the permit drops.
#[must_use = "a permit returns its bytes when dropped"]
pub struct Permit {
    bytes: u64,
    home: Option<Arc<dyn PermitHome>>,
}

impl Permit {
    /// A permit that belongs to no budget.
    pub fn detached(bytes: u64) -> Self {
        Self { bytes, home: None }
    }

    /// A permit that returns its bytes to `home`.
    pub fn new(bytes: u64, home: Arc<dyn PermitHome>) -> Self {
        Self {
            bytes,
            home: Some(home),
        }
    }

    /// The bytes this permit holds.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl fmt::Debug for Permit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Permit")
            .field("bytes", &self.bytes)
            .field("attached", &self.home.is_some())
            .finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(home) = self.home.take() {
            home.release(self.bytes);
        }
    }
}

/// A budget that never refuses: UnityPy's behaviour, and the default.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unbounded;

impl MemoryBudget for Unbounded {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::detached(bytes))
    }
}

/// A non-blocking budget with a fixed limit, counting what is alive.
#[derive(Debug)]
pub struct Counting {
    limit: u64,
    used: Arc<CountingHome>,
}

#[derive(Debug, Default)]
struct CountingHome {
    used: AtomicU64,
}

impl PermitHome for CountingHome {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl Counting {
    /// A budget of `limit` bytes.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: Arc::new(CountingHome::default()),
        }
    }

    /// Bytes reserved and not yet returned.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used.used.load(Ordering::Acquire)
    }
}

impl MemoryBudget for Counting {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let refused = BudgetExceeded {
            requested: bytes,
            what,
        };
        self.used
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.limit)
            })
            .map_err(|_| refused)?;
        Ok(Permit::new(bytes, self.used.clone()))
    }
}

struct Budgeted {
    buf: Vec<u8>,
    _permit: Permit,
}

impl AsRef<[u8]> for Budgeted {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

/// Turns a buffer and the permit that paid for it into `Bytes`. The permit is
/// released when the last slice of the buffer drops.
#[must_use]
pub fn budgeted_bytes(buf: Vec<u8>, permit: Permit) -> Bytes {
    Bytes::from_owner(Budgeted {
        buf,
        _permit: permit,
    })
}

#[cfg(test)]
mod tests {
    use super::{Counting, MemoryBudget, Reservation, budgeted_bytes};

    #[test]
    fn a_permit_lives_as_long_as_its_last_slice() {
        let budget = Counting::new(100);
        let permit = budget.try_reserve(60, Reservation::BundleData).unwrap();
        assert!(budget.try_reserve(50, Reservation::BundleData).is_err());
        let whole = budgeted_bytes(vec![7; 60], permit);
        let slice = whole.slice(10..20);
        drop(whole);
        assert_eq!(budget.used(), 60);
        drop(slice);
        assert_eq!(budget.used(), 0);
        assert!(budget.try_reserve(100, Reservation::Inflate).is_ok());
    }
}
