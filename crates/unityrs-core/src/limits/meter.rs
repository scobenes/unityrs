//! The [`Meter`] of one open of an input, the memory account every open of
//! the input shares, what each lazy computation was granted on its own thread
//! ([`GrantScope`]), and the [`Charge`] each grant is.

use std::fmt;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{ChargeError, Limits, heap_cost};
use crate::budget::{MemoryBudget, Permit, Reservation, Unbounded};
use crate::error::{LimitError, LimitKind};

/// The bytes an `Arc` of a `T` of `size` bytes asks for: the value and the
/// two reference counts.
const fn arc_bytes(size: usize) -> u64 {
    size as u64 + 2 * size_of::<usize>() as u64
}

/// What every open of one input holds: the memory account its meters share.
struct Account {
    used: AtomicU64,
    limit: u64,
    budget: Arc<dyn MemoryBudget>,
    what: Reservation,
}

/// One open of an input: what the input holds, against
/// [`Limits::max_input_memory`], and what this open has done, against
/// [`Limits::max_decode_work`] (module docs). Thread-safe: a lazily read
/// bundle's entries charge one meter from any thread. Shared as
/// `Arc<Meter>`; every [`Charge`] keeps it alive.
pub struct Meter {
    /// The input's memory, shared by every open of it ([`Self::reopen`]).
    account: Arc<Account>,
    /// Work done by this open ([`Limits::max_decode_work`]).
    work: AtomicU64,
    work_limit: u64,
    /// Every byte this meter ever granted, never returned: what a lazy
    /// computation is measured by ([`crate::once::get_or_publish_result`]).
    granted: AtomicU64,
}

const _: () = crate::send_sync::<Meter>();

impl fmt::Debug for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Meter")
            .field("used", &self.used())
            .field("limit", &self.limit())
            .field("decoded", &self.decoded())
            .field("what", &self.reservation())
            .finish_non_exhaustive()
    }
}

impl Meter {
    /// The heap an input's first meter takes itself, outside every charge,
    /// in glibc chunks ([`heap_cost`]): the meter and the memory account, each
    /// in its `Arc` (plan section 3.15's constant per input). Each
    /// [`Self::reopen`] adds [`Self::REOPEN_HEAP`].
    pub const OWN_HEAP: u64 = Self::REOPEN_HEAP + heap_cost(arc_bytes(size_of::<Account>()));

    /// The heap one more open's meter takes, in glibc chunks.
    pub const REOPEN_HEAP: u64 = heap_cost(arc_bytes(size_of::<Self>()));

    /// A meter of `limit` bytes whose charges are booked with `budget` as
    /// `what`, with no bound on decode work.
    #[must_use]
    pub fn new(limit: u64, budget: Arc<dyn MemoryBudget>, what: Reservation) -> Arc<Self> {
        Self::with_decode_work(limit, u64::MAX, budget, what)
    }

    /// A meter of `limit` bytes that also counts decode work up to
    /// `max_decode_work` bytes ([`Self::count_decode`]).
    #[must_use]
    pub fn with_decode_work(
        limit: u64,
        max_decode_work: u64,
        budget: Arc<dyn MemoryBudget>,
        what: Reservation,
    ) -> Arc<Self> {
        Arc::new(Self {
            account: Arc::new(Account {
                used: AtomicU64::new(0),
                limit,
                budget,
                what,
            }),
            work: AtomicU64::new(0),
            work_limit: max_decode_work,
            granted: AtomicU64::new(0),
        })
    }

    /// The meter of one input loaded with `limits` and `budget`: its memory
    /// bound is [`Limits::max_input_memory`] and its decode-work bound
    /// [`Limits::max_decode_work`].
    #[must_use]
    pub fn for_input(
        limits: &Limits,
        budget: Arc<dyn MemoryBudget>,
        what: Reservation,
    ) -> Arc<Self> {
        Self::with_decode_work(
            limits.max_input_memory,
            limits.max_decode_work,
            budget,
            what,
        )
    }

    /// A meter for opening the same input again: it charges the same memory
    /// account, so everything every open holds counts against one
    /// [`Limits::max_input_memory`], and its work starts from zero with the
    /// same bound. The Environment opens an evicted dependency again on it
    /// (plan section 3.13). The old meter keeps its own count, so a handle
    /// of the old open that something still holds (a nested bundle a
    /// resource read kept) goes on counting against the old allowance, which
    /// nothing resets, and every open's work stays bounded.
    #[must_use]
    pub fn reopen(&self) -> Arc<Self> {
        Arc::new(Self {
            account: self.account.clone(),
            work: AtomicU64::new(0),
            work_limit: self.work_limit,
            granted: AtomicU64::new(0),
        })
    }

    /// Whether `other` charges the same input's memory (one of them is a
    /// [`Self::reopen`] of the other, or they are the same meter).
    #[must_use]
    pub fn same_input(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.account, &other.account)
    }

    /// Counts `bytes` of this open's work, refused when it would pass the
    /// bound ([`LimitKind::DecodeWork`]): one atomic check-and-add, so work
    /// counted at once from several threads never passes the bound. Work is
    /// never returned while the open lives: once refused, every later
    /// decode on this meter is refused too, whatever its charges do. A
    /// [`crate::budget::DecodeBuf`] counts every write of a block's segment:
    /// one that needs a growth after the meter and the budget granted its
    /// memory and before it allocates, so a refused growth counts nothing,
    /// and a growth the count refuses returns its charge and allocates
    /// nothing.
    pub fn count_decode(&self, bytes: u64) -> Result<(), ChargeError> {
        let limit = self.work_limit;
        self.work
            .try_update(Ordering::AcqRel, Ordering::Acquire, |done| {
                done.checked_add(bytes).filter(|t| *t <= limit)
            })
            .map(|_| ())
            .map_err(|done| self.decode_refused(done, bytes))
    }

    /// Takes back `bytes` counted by [`Self::count_decode`] for a growth
    /// the allocator then failed (nothing was allocated).
    pub(crate) fn uncount_decode(&self, bytes: u64) {
        let _ = self
            .work
            .try_update(Ordering::AcqRel, Ordering::Acquire, |done| {
                Some(done.saturating_sub(bytes))
            });
    }

    /// Counts `bytes` of work already done, unchecked: what a lazy
    /// computation that ended unkept was granted
    /// ([`crate::once::get_or_publish_result`]). The work is done whatever
    /// the bound says; what follows is refused ([`Self::work_used_up`]).
    pub(crate) fn add_work(&self, bytes: u64) {
        let _ = self
            .work
            .try_update(Ordering::AcqRel, Ordering::Acquire, |done| {
                Some(done.saturating_add(bytes))
            });
    }

    /// The refusal a new piece of work gets once this open's work has passed
    /// its allowance, or `None` while it has not. The allowance is inclusive,
    /// as [`Self::count_decode`] admits work: work that lands exactly on it
    /// is within it, and what follows is refused by `count_decode` itself,
    /// its note naming the work it would reach. Only work counted past the
    /// bound ([`Self::add_work`]: a detection's refused chunk, what a
    /// refused computation was granted) passes it, so the note's work is
    /// always past the bound (`limit DecodeWork: <work> > <bound>`).
    #[must_use]
    pub fn work_used_up(&self) -> Option<LimitError> {
        let done = self.decoded();
        (done > self.work_limit).then_some(LimitError {
            kind: LimitKind::DecodeWork,
            value: done,
            max: self.work_limit,
        })
    }

    fn decode_refused(&self, done: u64, bytes: u64) -> ChargeError {
        ChargeError::Limit(LimitError {
            kind: LimitKind::DecodeWork,
            value: done.saturating_add(bytes),
            max: self.work_limit,
        })
    }

    /// Work counted so far by this open.
    #[must_use]
    pub fn decoded(&self) -> u64 {
        self.work.load(Ordering::Acquire)
    }

    /// Every byte this meter has granted since it was made, returned or not.
    #[must_use]
    pub fn granted(&self) -> u64 {
        self.granted.load(Ordering::Acquire)
    }

    /// A meter with no bound and no budget, for data that is not an input's:
    /// a TPK's trees, a caller's `nodes=` rows, tests.
    #[must_use]
    pub fn unlimited() -> Arc<Self> {
        Self::new(u64::MAX, Arc::new(Unbounded), Reservation::Input)
    }

    /// Bytes charged to the input and not yet returned, by every open of it.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.account.used.load(Ordering::Acquire)
    }

    /// The bound.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.account.limit
    }

    /// How the budget books this meter's charges.
    #[must_use]
    pub fn reservation(&self) -> Reservation {
        self.account.what
    }

    /// Charges `bytes`: against the bound, then with the budget. A refusal
    /// charges nothing.
    pub fn charge(self: &Arc<Self>, bytes: u64) -> Result<Charge, ChargeError> {
        let permit = self.reserve(bytes)?;
        Ok(Charge {
            meter: self.clone(),
            bytes,
            permit,
            more: Vec::new(),
        })
    }

    fn reserve(&self, bytes: u64) -> Result<Permit, ChargeError> {
        let account = &self.account;
        let limit = account.limit;
        account
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|t| *t <= limit)
            })
            .map_err(|used| LimitError {
                kind: LimitKind::InputMemory,
                value: used.saturating_add(bytes),
                max: limit,
            })?;
        let permit = account
            .budget
            .try_reserve(bytes, account.what)
            .map_err(|e| {
                self.refund(bytes);
                ChargeError::Budget(e)
            })?;
        let _ = self
            .granted
            .try_update(Ordering::AcqRel, Ordering::Acquire, |g| {
                Some(g.saturating_add(bytes))
            });
        GrantScope::credit(self, bytes);
        Ok(permit)
    }

    fn refund(&self, bytes: u64) {
        // Saturating: a refund never takes the count below zero.
        let _ = self
            .account
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                Some(used.saturating_sub(bytes))
            });
    }
}

/// How many lazy computations one thread tracks at once ([`GrantScope`]):
/// more than any nesting the core reaches (an entry parse inside a nested
/// bundle's, a compile inside a read, bounded by
/// [`Limits::max_container_depth`] and the walker's limits). A deeper scope
/// falls back to its meter's total grants, which errs towards stopping
/// early.
const GRANT_SCOPES: usize = 64;

/// The lazy computations running on this thread, innermost last: the
/// address of the meter each measures and what that meter granted on this
/// thread since the computation started. A fixed array, so tracking
/// allocates nothing (a counting-allocator test sees every heap byte).
struct GrantFrames {
    frames: [(usize, u64); GRANT_SCOPES],
    len: usize,
}

thread_local! {
    static GRANTS: std::cell::RefCell<GrantFrames> = const {
        std::cell::RefCell::new(GrantFrames {
            frames: [(0, 0); GRANT_SCOPES],
            len: 0,
        })
    };
}

/// What one lazy computation was granted by its meter on its own thread
/// while it ran ([`crate::once::get_or_publish_result`]): grants that other
/// threads made on the same meter meanwhile are theirs, not its (round 6:
/// the meter's total since the computation started was counted, so other
/// threads' successful work used up a failing computation's allowance).
/// Scopes nest; a grant counts for every open scope of its meter on the
/// thread, a nested computation's grants included.
pub(crate) struct GrantScope {
    /// The scope's frame index, or `None` past [`GRANT_SCOPES`] (then
    /// `fallback` holds the meter's total grants at the start).
    depth: Option<usize>,
    fallback: u64,
}

impl GrantScope {
    /// Opens a scope measuring `meter`'s grants on this thread.
    pub(crate) fn enter(meter: &Meter) -> Self {
        let addr = std::ptr::from_ref(meter).addr();
        let depth = GRANTS
            .try_with(|g| {
                let mut g = g.borrow_mut();
                let at = g.len;
                let frame = g.frames.get_mut(at)?;
                *frame = (addr, 0);
                g.len = at + 1;
                Some(at)
            })
            .ok()
            .flatten();
        Self {
            depth,
            fallback: meter.granted(),
        }
    }

    /// Closes the scope: what `meter` (the one it was opened on) granted on
    /// this thread while it was open.
    pub(crate) fn finish(self, meter: &Meter) -> u64 {
        match self.depth {
            Some(at) => GRANTS
                .try_with(|g| g.borrow().frames.get(at).map_or(0, |f| f.1))
                .unwrap_or(0),
            None => meter.granted().saturating_sub(self.fallback),
        }
        // `self` drops here and pops its frame.
    }

    /// Adds `bytes` granted by `meter` to every open scope of it on this
    /// thread.
    fn credit(meter: &Meter, bytes: u64) {
        let addr = std::ptr::from_ref(meter).addr();
        let _ = GRANTS.try_with(|g| {
            let mut g = g.borrow_mut();
            let len = g.len;
            for frame in g.frames.iter_mut().take(len) {
                if frame.0 == addr {
                    frame.1 = frame.1.saturating_add(bytes);
                }
            }
        });
    }
}

impl Drop for GrantScope {
    fn drop(&mut self) {
        if let Some(at) = self.depth {
            // Scopes close in reverse order of opening (a stack frame each),
            // so this is the innermost one.
            let _ = GRANTS.try_with(|g| g.borrow_mut().len = at);
        }
    }
}

/// Bytes charged to a [`Meter`] (and booked with its budget), returned when
/// the charge drops. It rides inside whatever it paid for.
///
/// **Bookkeeping.** A charge holds one [`Permit`] for its own meter, and
/// every charge it absorbs from that meter merges into it (bytes added,
/// permits merged by home), so a value built from any number of charged
/// tables keeps one record, however its budget books it. A charge absorbed
/// from *another* meter is kept as one record per distinct meter. Neither
/// grows with the input: one meter and one budget home (every budget of the
/// core: [`Unbounded`], [`crate::budget::Counting`]) allocate no record at
/// all (`tests/memory_model.rs` checks this under `Counting`).
#[must_use = "a charge returns its bytes when dropped"]
pub struct Charge {
    meter: Arc<Meter>,
    bytes: u64,
    permit: Permit,
    /// Absorbed charges of other meters, one per meter.
    more: Vec<Charge>,
}

const _: () = crate::send_sync::<Charge>();

impl fmt::Debug for Charge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Charge")
            .field("bytes", &self.bytes())
            .field("more", &self.more.len())
            .finish_non_exhaustive()
    }
}

impl Charge {
    /// A charge of nothing on `meter`, to grow.
    pub fn zero(meter: &Arc<Meter>) -> Self {
        Self {
            meter: meter.clone(),
            bytes: 0,
            permit: Permit::detached(0),
            more: Vec::new(),
        }
    }

    /// The bytes held, absorbed charges included.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.more
            .iter()
            .fold(self.bytes, |n, c| n.saturating_add(c.bytes()))
    }

    /// The meter this charge is on.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }

    /// Charges `bytes` more to the same meter. A refusal changes nothing.
    pub fn grow(&mut self, bytes: u64) -> Result<(), ChargeError> {
        let permit = self.meter.reserve(bytes)?;
        self.permit.absorb(permit);
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }

    /// Returns up to `bytes` of this meter's part now (what a buffer no
    /// longer holds).
    pub fn release(&mut self, bytes: u64) {
        let n = bytes.min(self.bytes);
        self.bytes -= n;
        self.meter.refund(n);
        self.permit.shrink_to(self.bytes);
    }

    /// Takes `other` in: its bytes are returned when this charge drops. A
    /// charge of this meter merges into this one; a charge of another meter
    /// merges into the record this charge keeps for that meter (type docs).
    pub fn absorb(&mut self, mut other: Charge) {
        for c in std::mem::take(&mut other.more) {
            self.absorb(c);
        }
        if Arc::ptr_eq(&self.meter, &other.meter) {
            self.bytes = self.bytes.saturating_add(other.bytes);
            other.bytes = 0;
            self.permit
                .absorb(std::mem::replace(&mut other.permit, Permit::detached(0)));
        } else if other.bytes > 0 {
            match self
                .more
                .iter_mut()
                .find(|c| Arc::ptr_eq(&c.meter, &other.meter))
            {
                Some(kept) => kept.absorb(other),
                None => self.more.push(other),
            }
        }
    }

    /// How many records this charge keeps besides its own: one per other
    /// meter it absorbed, plus its permit's ([`Permit::records`]).
    #[must_use]
    pub fn records(&self) -> usize {
        self.more
            .iter()
            .fold(self.permit.records(), |n, c| n + 1 + c.records())
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        // The permit drops after this, returning the budget's side.
        self.meter.refund(self.bytes);
    }
}
