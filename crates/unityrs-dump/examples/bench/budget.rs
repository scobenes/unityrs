//! The bench's budgets: `Unbounded` (the default context's), one shared
//! `Counting` (a batch engine's), or `Tracked`, which counts every live
//! charge per input and for the process and keeps each one's peak.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{
    BudgetExceeded, Counting, MemoryBudget, Permit, PermitHome, Reservation, Unbounded,
};

/// Which budget the inputs' meters book with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetKind {
    /// [`Unbounded`]: `FileContext::unitypy()`'s, the dump's.
    Unbounded,
    /// One [`Counting`] with no limit, shared by every input.
    Counting,
    /// A [`Tracker`] per input, each adding to the process's.
    Tracked,
}

impl BudgetKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "unbounded" => Some(Self::Unbounded),
            "counting" => Some(Self::Counting),
            "tracked" => Some(Self::Tracked),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Unbounded => "unbounded",
            Self::Counting => "counting",
            Self::Tracked => "tracked",
        }
    }

    /// The budget every input shares, or `None` for one per input
    /// ([`Self::Tracked`]).
    pub fn shared(self) -> Option<Arc<dyn MemoryBudget>> {
        match self {
            Self::Unbounded => Some(Arc::new(Unbounded)),
            Self::Counting => Some(Arc::new(Counting::new(u64::MAX))),
            Self::Tracked => None,
        }
    }
}

/// Bytes reserved and not yet released, the most at once, and the tracker
/// every reservation is added to as well (the process's).
#[derive(Debug, Default)]
pub struct Tracker {
    used: AtomicU64,
    peak: AtomicU64,
    parent: Option<Arc<Tracker>>,
}

impl Tracker {
    /// A tracker whose reservations also count in `parent`.
    pub fn child(parent: &Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            parent: Some(parent.clone()),
            ..Self::default()
        })
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }

    fn add(&self, bytes: u64) {
        let now = self
            .used
            .fetch_add(bytes, Ordering::AcqRel)
            .saturating_add(bytes);
        self.peak.fetch_max(now, Ordering::AcqRel);
        if let Some(p) = &self.parent {
            p.add(bytes);
        }
    }

    /// The budget that books with this tracker.
    pub fn budget(self: &Arc<Self>) -> Arc<dyn MemoryBudget> {
        Arc::new(Booked(self.clone()))
    }
}

impl PermitHome for Tracker {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        if let Some(p) = &self.parent {
            p.release(bytes);
        }
    }
}

/// A [`MemoryBudget`] that never refuses and books with its tracker.
#[derive(Debug)]
struct Booked(Arc<Tracker>);

impl MemoryBudget for Booked {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        self.0.add(bytes);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}
