//! Request slots and blocking-operation tokens. All storage is prepared once.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

const REQUEST: u64 = 1;
const LOOKUP: u64 = 2;
const WRITE: u64 = 4;
const CHARGE: u64 = 8;
/// Each permit's fixed network allowance (`batch.md` 5.3).
pub const NETWORK_ALLOWANCE: u64 = 4 * 1_024 * 1_024;

struct Slot {
    state: AtomicU64,
    bytes: AtomicU64,
    waiter: Mutex<Option<Waker>>,
}

/// Preallocated request slots; a lookup outliving its request quarantines one.
pub struct PermitPool {
    slots: Vec<Arc<Slot>>,
}
impl PermitPool {
    /// Startup storage belongs to measured phase-1 state. No slot is allocated
    /// on acquisition. Zero slots cannot make progress and are refused.
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        if capacity == 0 {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| std::io::ErrorKind::OutOfMemory)?;
        for _ in 0..capacity {
            slots.push(Arc::new(Slot {
                state: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
                waiter: Mutex::new(None),
            }));
        }
        Ok(Self { slots })
    }
    /// Nonblocking admission. A live data buffer also keeps its slot occupied.
    #[must_use]
    pub fn acquire(&self) -> Option<RequestPermit> {
        self.slots.iter().find_map(|slot| {
            if slot
                .state
                .compare_exchange(0, REQUEST, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                Some(RequestPermit { slot: slot.clone() })
            } else {
                None
            }
        })
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let mut result = Snapshot::default();
        for slot in &self.slots {
            let state = slot.state.load(Ordering::Acquire);
            result.occupied += usize::from(state != 0);
            result.outstanding += usize::from(state & (LOOKUP | WRITE) != 0);
        }
        result
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub occupied: usize,
    pub outstanding: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Lookup,
    Write,
}
impl Operation {
    fn bit(self) -> u64 {
        match self {
            Self::Lookup => LOOKUP,
            Self::Write => WRITE,
        }
    }
}
/// A request retains this permit through all its hops and tries.
pub struct RequestPermit {
    slot: Arc<Slot>,
}
impl RequestPermit {
    /// The closure owns the returned token, never its join handle. Dropping a
    /// timed-out future therefore cannot return a running lookup's occupancy.
    pub fn begin(&mut self, operation: Operation) -> Option<OperationToken> {
        self.slot
            .state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & (REQUEST | LOOKUP | WRITE) == REQUEST).then_some(state | operation.bit())
            })
            .ok()?;
        Some(OperationToken {
            slot: self.slot.clone(),
            operation,
        })
    }
    /// Register before rechecking, so a returning resolver cannot lose a wake.
    pub fn poll_available(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut waiter = self
            .slot
            .waiter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.slot.state.load(Ordering::Acquire) & (LOOKUP | WRITE) == 0 {
            *waiter = None;
            Poll::Ready(())
        } else {
            if waiter.as_ref().is_none_or(|w| !w.will_wake(cx.waker())) {
                *waiter = Some(cx.waker().clone());
            }
            Poll::Pending
        }
    }
    /// Reserve counted engine storage, or a conservative third-party envelope.
    /// The latter must also pass the focused allocator tests on the exact graph.
    #[must_use]
    pub fn charge(&self, bytes: u64) -> Option<Charge> {
        charge(&self.slot, bytes)
    }
    #[must_use]
    pub fn charged(&self) -> u64 {
        self.slot.bytes.load(Ordering::Acquire)
    }
}
impl Drop for RequestPermit {
    fn drop(&mut self) {
        self.slot.state.fetch_and(!REQUEST, Ordering::AcqRel);
    }
}
/// Exactly one of these may be outstanding per slot, including after timeout.
pub struct OperationToken {
    slot: Arc<Slot>,
    operation: Operation,
}
impl Drop for OperationToken {
    fn drop(&mut self) {
        self.slot
            .state
            .fetch_and(!self.operation.bit(), Ordering::AcqRel);
        let wake = self
            .slot
            .waiter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
/// Keeps the original permit occupied while a moved buffer still owns memory.
pub struct Charge {
    slot: Arc<Slot>,
    bytes: u64,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.slot.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.slot.state.fetch_sub(CHARGE, Ordering::AcqRel);
    }
}

impl Charge {
    pub(crate) fn additional(&self, bytes: u64) -> Option<Self> {
        charge(&self.slot, bytes)
    }
}
fn charge(slot: &Arc<Slot>, bytes: u64) -> Option<Charge> {
    if bytes == 0 {
        return None;
    }
    slot.bytes
        .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes)
                .filter(|&next| next <= NETWORK_ALLOWANCE)
        })
        .ok()?;
    slot.state.fetch_add(CHARGE, Ordering::AcqRel);
    Some(Charge {
        slot: slot.clone(),
        bytes,
    })
}
