//! Request slots and blocking-operation tokens. All storage is prepared once.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

const REQUEST: u64 = 1;
const LOOKUP: u64 = 2;
const WRITE: u64 = 4;
const CHARGE: u64 = 8;
/// The finisher's permits' first socket-table entry; the controller's start
/// at 0. Both runtimes' `download_concurrency` is at most 256.
pub const FINISHER_BASE: usize = 257;
/// Each permit's fixed network allowance (`batch.md` 5.3).
pub const NETWORK_ALLOWANCE: u64 = 4 * 1_024 * 1_024;

struct Slot {
    /// This permit's socket-table entry (`sys::reset`): its pool's base
    /// plus its position. One table entry per permit, never shared.
    index: usize,
    state: AtomicU64,
    bytes: AtomicU64,
    waiter: Mutex<Option<Waker>>,
    /// The pool's waiter, shared by every slot: whatever frees a slot or
    /// ends an operation (a permit's, a token's or a charge's drop) wakes
    /// it, so a caller waiting for a permit or for idle never misses a
    /// return, a quarantined lookup's included.
    pool: Arc<Mutex<Option<Waker>>>,
}
impl Slot {
    /// After a state change that may free this slot or end its operation.
    fn released(&self) {
        let waiter = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(waker) = waiter.as_ref() {
            waker.wake_by_ref();
        }
    }
}
/// Register `cx` as the pool's waiter before the caller rechecks the slots.
fn register(pool: &Mutex<Option<Waker>>, cx: &Context<'_>) {
    let mut waiter = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if waiter.as_ref().is_none_or(|w| !w.will_wake(cx.waker())) {
        *waiter = Some(cx.waker().clone());
    }
}

/// Preallocated request slots; a lookup outliving its request quarantines one.
pub struct PermitPool {
    slots: Vec<Arc<Slot>>,
    waiter: Arc<Mutex<Option<Waker>>>,
}
impl PermitPool {
    /// Process startup storage follows the per-pool `m_pool` slope (one slot
    /// per possible blocking operation), not fixed `c_base`. No slot is
    /// allocated on acquisition. Zero slots cannot make progress.
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        Self::with_base(0, capacity)
    }
    /// The controller's pool takes base 0 and the finisher's [`FINISHER_BASE`],
    /// whatever `download_concurrency` is, so the two runtimes' permits never
    /// share a socket-table entry.
    pub fn with_base(base: usize, capacity: usize) -> std::io::Result<Self> {
        if capacity == 0
            || base
                .checked_add(capacity)
                .is_none_or(|end| end > crate::sys::reset::SOCKET_CAPACITY)
        {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| std::io::ErrorKind::OutOfMemory)?;
        let waiter = Arc::new(Mutex::new(None));
        for index in base..base + capacity {
            slots.push(Arc::new(Slot {
                index,
                state: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
                waiter: Mutex::new(None),
                pool: waiter.clone(),
            }));
        }
        Ok(Self { slots, waiter })
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
    /// [`Self::acquire`], or `Pending` with `cx` registered as the pool's
    /// waiter first, so the drop that frees a slot (a request's end, or a
    /// quarantined lookup's return) wakes it.
    pub fn poll_acquire(&self, cx: &mut Context<'_>) -> Poll<RequestPermit> {
        register(&self.waiter, cx);
        self.acquire().map_or(Poll::Pending, Poll::Ready)
    }
    /// Ready once no slot has a lookup or write outstanding: every blocking
    /// operation has returned, quarantined lookups included. Registers `cx`
    /// as the pool's waiter before checking, so a token's return wakes it.
    pub fn poll_idle(&self, cx: &mut Context<'_>) -> Poll<()> {
        register(&self.waiter, cx);
        let busy = self
            .slots
            .iter()
            .any(|slot| slot.state.load(Ordering::Acquire) & (LOOKUP | WRITE) != 0);
        if busy { Poll::Pending } else { Poll::Ready(()) }
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
    /// This permit's socket-table entry.
    #[must_use]
    pub fn index(&self) -> usize {
        self.slot.index
    }
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
        self.slot.released();
    }
}
/// Exactly one of these may be outstanding per slot, including after timeout.
pub struct OperationToken {
    slot: Arc<Slot>,
    operation: Operation,
}
impl OperationToken {
    pub(crate) fn belongs_to(&self, permit: &RequestPermit) -> bool {
        self.operation == Operation::Lookup && Arc::ptr_eq(&self.slot, &permit.slot)
    }
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
        self.slot.released();
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
        #[cfg(feature = "test-hooks")]
        crate::run::hooks::memory::network(self.bytes, false);
        self.slot.state.fetch_sub(CHARGE, Ordering::AcqRel);
        self.slot.released();
    }
}

impl Charge {
    /// Replace a completed phase's reservation without a second owner. The
    /// state keeps its existing charge count; Drop releases the grown total.
    pub(crate) fn grow(&mut self, bytes: u64) -> Option<()> {
        let total = self.bytes.checked_add(bytes)?;
        self.slot
            .bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|&next| next <= NETWORK_ALLOWANCE)
            })
            .ok()?;
        self.bytes = total;
        #[cfg(feature = "test-hooks")]
        crate::run::hooks::memory::network(bytes, true);
        Some(())
    }
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
    #[cfg(feature = "test-hooks")]
    crate::run::hooks::memory::network(bytes, true);
    Some(Charge {
        slot: slot.clone(),
        bytes,
    })
}
