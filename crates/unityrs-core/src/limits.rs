//! Hard bounds on what untrusted input can make the library allocate or do,
//! and the one meter that enforces the memory bound.
//!
//! UnityPy has none of these: where it would exhaust memory or the C stack,
//! unityrs stops with [`crate::Error::Limit`] (or the walker's `other` kind) and
//! `PARITY.md` lists the case as a safety divergence (plan section 3.15).
//!
//! **One meter per input.** Everything the core allocates in proportion to a
//! size or count the input chose is charged to the input's [`Meter`] *before*
//! it is allocated: the input's own bytes when the core reads a path, every
//! block decode (a first decode, a lazy re-decode, a measuring decode, a
//! decode after eviction), every metadata table and typetree row of every
//! entry parse (an entry parsed twice, or two directory rows over the same
//! bytes, pay twice), every compiled tree, every parsed value and every
//! writer output. A charge the meter refuses allocates nothing. The charge is
//! a [`Charge`] that lives inside what it paid for, so the bytes return when
//! that drops: a [`crate::Parsed`] carries the charge for its values, a
//! decoded buffer's [`bytes::Bytes`] the charge for the buffer. So what one
//! input holds at any moment, retained values included, is at most
//! [`Limits::max_input_memory`] (plus the constant overhead plan section 3.15
//! states). A top-level load makes the meter; every file inside it (a
//! bundle's entries, nested bundles) charges the same one.
//!
//! **A meter is one open of its input.** Its memory is the input's: an
//! account every open of the input shares, so the bound holds across opens.
//! Its *work* ([`Limits::max_decode_work`]) is the open's own: the
//! Environment opens an evicted dependency again on [`Meter::reopen`], a new
//! meter on the same account with a fresh allowance, while a handle of the
//! old open that someone still holds (a nested bundle a resource read kept)
//! goes on counting against the old meter's, which nothing resets.
//!
//! **One accounting path.** The meter books every charge it grants with the
//! load's [`MemoryBudget`] as well ([`crate::FileContext::budget`]), so a
//! batch engine's budget sees exactly what every input holds, and a refusal
//! says which side refused: [`ChargeError::Limit`] (this input's bound) or
//! [`ChargeError::Budget`] (the shared budget, tagged with the meter's
//! [`Reservation`]).
//!
//! The other bounds are on work and nesting, per object read or per input.
//!
//! Owner: the scaffold (implemented). Packages read these fields and charge
//! through these types; a new limit is added only through the integration
//! package.

use std::fmt;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use crate::error::{LimitError, LimitKind};

/// Bounds on allocation, recursion and work per input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// What the core may hold for one input at once, in bytes, charged to
    /// the input's [`Meter`] before every allocation whose size the input
    /// chooses (module docs). Default 8 GiB.
    pub max_input_memory: u64,
    /// The longest file [`crate::env::Source::Path`] reads, checked against
    /// the file's length before anything is read, and again as it is read
    /// ([`crate::env::read_path`]). Default 4 GiB.
    pub max_input_bytes: u64,
    /// One block's actual decoded output (blocks-info's included). LZMA
    /// decodes to the end of its input whatever blocks-info says
    /// (`helpers/CompressionHelper.py:17-44`), so a bomb is stopped here
    /// before the meter. Default 1 GiB.
    pub max_block_output: u64,
    /// The *work* one open of an input may cost, in bytes: every byte a
    /// block decode was granted room for, kept or not (a first decode, a
    /// measuring decode whose output is dropped, a re-decode of a shared
    /// block, a decode that fails after it wrote), and every byte the meter
    /// granted to a lazy computation that ended in an error its cell does
    /// not keep (a parse or compile the meter refused, which the next ask
    /// runs again: [`crate::once::get_or_publish_result`]). The meter bounds
    /// what is held; this bounds what is done, so an input cannot make its
    /// readers decode or recompute without end while the meter stays empty
    /// (thousands of blocks that each decode to the block cap, measured and
    /// dropped by a lazy read of the last entry; one type whose compile alone
    /// needs more than the meter, asked by every object of the type). A
    /// growth the meter or the budget refuses decodes nothing and counts
    /// nothing, so a refusal never uses the allowance up; a growth is counted
    /// atomically between its charge and its allocation, so decodes running
    /// at once never pass the bound ([`Meter::count_decode`]). It is counted
    /// per open: every file of one open shares its meter's count, and the
    /// Environment reopens an evicted dependency on [`Meter::reopen`], so
    /// eviction churn costs work in proportion to the reads that cause it
    /// and never exhausts a valid dependency. UnityPy decodes every block
    /// once; a lazy read of a bundle whose entries do not overlap decodes a
    /// block at most four times: measured, decoded for the entries wholly
    /// inside it, and decoded again for each of the two entries that
    /// straddle its edges (plan section 3.6, "Runs"). Default 32 GiB, four
    /// times [`Self::max_input_memory`].
    pub max_decode_work: u64,
    /// One gzip stream inflated by detection (rule 7, streamed into a fixed
    /// scratch buffer and never held). Default 4 GiB.
    pub max_inflate: u64,
    /// Containers nested in one input (a bundle entry that is itself a
    /// bundle, and so on): parsing one recurses. Default 16.
    pub max_container_depth: u32,
    /// Depth of a compiled typetree. Default 256.
    pub max_tree_depth: u32,
    /// Runtime nesting while reading a value, `ReferencedObject` data
    /// included (UnityPyBoost recurses on the C stack, unbounded). Default 512.
    pub max_value_depth: u32,
    /// Values built by one object read, each element of a primitive run
    /// included. This bounds the *work* of one read (the pure-Python reader
    /// can read the same bytes again after a negative length); their memory
    /// is the meter's. Default 2^26.
    pub max_values_per_object: u64,
    /// Array elements whose minimum size is 0 (a vector of empty structs with
    /// a huge count does no I/O per element). Default 2^20 per object.
    pub max_zero_size_elements: u64,
    /// Elements reserved before the bytes prove they exist. Default 2^16.
    pub max_prealloc: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_memory: 8 << 30,
            max_input_bytes: 4 << 30,
            max_block_output: 1 << 30,
            max_decode_work: 32 << 30,
            max_inflate: 4 << 30,
            max_container_depth: 16,
            max_tree_depth: 256,
            max_value_depth: 512,
            max_values_per_object: 1 << 26,
            max_zero_size_elements: 1 << 20,
            max_prealloc: 1 << 16,
        }
    }
}

/// Buckets a hash table is charged (or counted) per key: a doubling table
/// (hashbrown's, behind `std`'s `HashMap`) is at least 7/16 full once it has
/// grown, so it holds up to 16/7 buckets per key, and while it grows the old
/// table and the doubled one are both live, up to 24/7 buckets per key; four
/// cover both. A bucket is a key, a value and one control byte; the table's
/// fixed part (its control bytes' tail of one SIMD group, 16 bytes, and the
/// allocator's rounding of its one allocation) is a constant per table.
pub(crate) const HASH_TABLE_SHARES: u64 = 4;

/// glibc's default `M_MMAP_THRESHOLD`: a chunk of at least this many bytes
/// may be `mmap`ped (the threshold starts here and can only rise, which
/// serves more chunks from the heap, never fewer).
const MMAP_THRESHOLD: u64 = 128 << 10;

/// What one heap allocation of `bytes` costs, which is what a charge for it
/// is: glibc's chunk on x86-64. The request gets its 8-byte header and is
/// rounded to 16, with a 32-byte minimum (`request2size`); a chunk of
/// 128 KiB (glibc's default `M_MMAP_THRESHOLD`) or more may be `mmap`ped instead, which adds another
/// 8 bytes and rounds to a 4 KiB page (`sysmalloc`'s
/// `ALIGN_UP(nb + SIZE_SZ, pagesize)`), and that is the larger of the two.
/// So a charge covers the allocator's rounding too, and many small
/// allocations cannot hold more than they were charged. 0 for no allocation.
#[must_use]
pub const fn heap_cost(bytes: u64) -> u64 {
    if bytes == 0 {
        return 0;
    }
    let chunk = bytes.saturating_add(8 + 15) & !15;
    let chunk = if chunk < 32 { 32 } else { chunk };
    if chunk < MMAP_THRESHOLD {
        chunk
    } else {
        chunk.saturating_add(8 + 4095) & !4095
    }
}

/// A charge the meter or the budget refused. Nothing was allocated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChargeError {
    /// The input's own bound ([`Limits::max_input_memory`], or a buffer's
    /// cap such as [`Limits::max_block_output`]).
    #[error(transparent)]
    Limit(#[from] LimitError),
    /// The shared budget refused, or the allocator failed.
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
}

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
    /// [`crate::budget::DecodeBuf`] counts a growth after the meter and the
    /// budget granted its memory and before it allocates, so a refused
    /// growth counts nothing, and a growth the count refuses returns its
    /// charge and allocates nothing.
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

    /// The refusal a new piece of work gets once this open's allowance is
    /// used up (no byte of it left), or `None` while some is left.
    #[must_use]
    pub fn work_used_up(&self) -> Option<LimitError> {
        let done = self.decoded();
        (done >= self.work_limit).then_some(LimitError {
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

fn bytes_of<T>(n: usize) -> u64 {
    u64::try_from(n)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(size_of::<T>()).unwrap_or(u64::MAX))
}

/// Grows `vec`'s capacity to at least `need`, geometrically up to `max`
/// (never below `need`), charging the new allocation to `charge`'s meter
/// first. While the allocator moves the buffer both blocks exist, so both are
/// charged until it is done; then the old block's charge is returned. A
/// refused doubling retries at exactly `need` before it fails, so a meter
/// that fits the data is never refused by the growth policy.
pub(crate) fn grow_charged<T>(
    vec: &mut Vec<T>,
    charge: &mut Charge,
    need: usize,
    max: usize,
) -> Result<(), ChargeError> {
    grow_counted(vec, charge, need, max, 0)
}

/// [`grow_charged`], counting `work` bytes of the meter's work
/// ([`Meter::count_decode`]) once the growth's memory is granted and before
/// it is allocated: a growth the meter or the budget refuses counts nothing,
/// and one the work bound refuses returns its charge and allocates nothing.
/// The check and the count are one atomic step, so growths on several
/// threads never pass the bound together.
pub(crate) fn grow_counted<T>(
    vec: &mut Vec<T>,
    charge: &mut Charge,
    need: usize,
    max: usize,
    work: u64,
) -> Result<(), ChargeError> {
    let have = vec.capacity();
    if need <= have {
        return Ok(());
    }
    let doubled = have.saturating_mul(2).min(max).max(need);
    if doubled > need {
        match grow_exact(vec, charge, doubled, work) {
            Ok(()) => return Ok(()),
            // The work bound refuses the exact size as well.
            Err(ChargeError::Limit(l)) if l.kind == LimitKind::DecodeWork => {
                return Err(ChargeError::Limit(l));
            }
            Err(_) => {}
        }
    }
    grow_exact(vec, charge, need, work)
}

fn grow_exact<T>(
    vec: &mut Vec<T>,
    charge: &mut Charge,
    cap: usize,
    work: u64,
) -> Result<(), ChargeError> {
    let old = heap_cost(bytes_of::<T>(vec.capacity()));
    let new = heap_cost(bytes_of::<T>(cap));
    let mut extra = charge.meter().charge(new)?;
    if work > 0 {
        // `extra` drops on a refusal: the memory granted is returned.
        charge.meter().count_decode(work)?;
    }
    let additional = cap.saturating_sub(vec.len());
    if vec.try_reserve_exact(additional).is_err() {
        // `extra` drops here: nothing stays charged or counted for a failed
        // allocation.
        if work > 0 {
            charge.meter().uncount_decode(work);
        }
        return Err(ChargeError::Budget(BudgetExceeded {
            requested: new,
            what: charge.meter().reservation(),
        }));
    }
    let got = heap_cost(bytes_of::<T>(vec.capacity()));
    if got > new && extra.grow(got - new).is_err() {
        vec.shrink_to(cap);
    }
    charge.release(old);
    charge.absorb(extra);
    Ok(())
}

/// A `Vec` whose every growth is charged to a [`Meter`] before it happens
/// (its capacity, at [`heap_cost`]). Parsers build every table this way, so
/// a table can never hold more than the meter granted, however many records
/// the input declares; [`Self::into_parts`] hands the charge on to whatever
/// keeps the table.
pub struct MeteredVec<T> {
    vec: Vec<T>,
    charge: Charge,
}

impl<T: fmt::Debug> fmt::Debug for MeteredVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeteredVec")
            .field("vec", &self.vec)
            .field("charge", &self.charge)
            .finish()
    }
}

impl<T> MeteredVec<T> {
    /// An empty vector charging `meter`.
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            vec: Vec::new(),
            charge: Charge::zero(meter),
        }
    }

    /// An empty vector with room for exactly `n`, charged first.
    pub fn with_capacity(meter: &Arc<Meter>, n: usize) -> Result<Self, ChargeError> {
        let mut v = Self::new(meter);
        v.reserve_exact(n)?;
        Ok(v)
    }

    /// Wraps a vector the caller already allocated (a TPK's rows, a test's),
    /// charging its capacity now. Parsers of input never use this: they grow
    /// a `MeteredVec` from empty.
    pub fn from_vec(vec: Vec<T>, meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        let charge = meter.charge(heap_cost(bytes_of::<T>(vec.capacity())))?;
        Ok(Self { vec, charge })
    }

    /// The elements.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.vec
    }

    /// The elements, mutably (the length cannot grow through this).
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.vec
    }

    /// Number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    /// The allocation, in elements.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    /// The charge for the allocation.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// Room for `additional` more, growing geometrically; charged first.
    pub fn reserve(&mut self, additional: usize) -> Result<(), ChargeError> {
        let need = self.vec.len().saturating_add(additional);
        grow_charged(&mut self.vec, &mut self.charge, need, usize::MAX)
    }

    /// Room for exactly `additional` more; charged first.
    pub fn reserve_exact(&mut self, additional: usize) -> Result<(), ChargeError> {
        let need = self.vec.len().saturating_add(additional);
        grow_charged(&mut self.vec, &mut self.charge, need, need)
    }

    /// Appends `value`, charging any growth first.
    pub fn push(&mut self, value: T) -> Result<(), ChargeError> {
        self.reserve(1)?;
        self.vec.push(value);
        Ok(())
    }

    /// Appends `value` with `charge`, the charge for the heap it owns, which
    /// this vector keeps from then on (a built value going into its parent,
    /// [`crate::value::Built`]). A refused growth drops both together.
    pub(crate) fn push_charged(&mut self, value: T, charge: Charge) -> Result<(), ChargeError> {
        self.reserve(1)?;
        self.vec.push(value);
        self.charge.absorb(charge);
        Ok(())
    }

    /// Keeps `charge` with this vector's own: it returns when they do.
    pub(crate) fn keep_charge(&mut self, charge: Charge) {
        self.charge.absorb(charge);
    }

    /// Cuts to `len` elements; the allocation and its charge stay.
    pub fn truncate(&mut self, len: usize) {
        self.vec.truncate(len);
    }

    /// The vector and its charge, which must live as long as the vector's
    /// allocation does.
    pub fn into_parts(self) -> (Vec<T>, Charge) {
        (self.vec, self.charge)
    }

    /// The elements as a boxed slice, and its charge. A vector with spare
    /// capacity is moved into an exact allocation first, which is charged
    /// before it is made (both blocks exist while the allocator moves it),
    /// so a refusal leaves nothing allocated beyond what was charged.
    pub fn into_boxed_slice(self) -> Result<(Box<[T]>, Charge), ChargeError> {
        let (mut vec, mut charge) = (self.vec, self.charge);
        if vec.capacity() > vec.len() {
            let smaller = charge.meter().charge(heap_cost(bytes_of::<T>(vec.len())))?;
            let old = heap_cost(bytes_of::<T>(vec.capacity()));
            vec.shrink_to_fit();
            charge.release(old);
            charge.absorb(smaller);
        }
        Ok((vec.into_boxed_slice(), charge))
    }
}

impl<T: Clone> MeteredVec<T> {
    /// Appends a copy of `items`, charging any growth first.
    pub fn extend_from_slice(&mut self, items: &[T]) -> Result<(), ChargeError> {
        self.reserve(items.len())?;
        self.vec.extend_from_slice(items);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Charge, ChargeError, Limits, Meter, MeteredVec, heap_cost};
    use crate::budget::{Counting, Reservation, Unbounded};
    use crate::error::LimitKind;

    #[test]
    fn a_refused_charge_charges_nothing_and_drops_return_everything() {
        let meter = Meter::new(100, Arc::new(Unbounded), Reservation::Input);
        let a = meter.charge(60).unwrap();
        let e = meter.charge(41).unwrap_err();
        assert_eq!(
            e,
            ChargeError::Limit(crate::LimitError {
                kind: LimitKind::InputMemory,
                value: 101,
                max: 100
            })
        );
        assert_eq!(meter.used(), 60, "a refused charge adds nothing");
        let mut b = meter.charge(40).unwrap();
        assert!(b.grow(1).is_err());
        b.release(15);
        assert_eq!(meter.used(), 85);
        drop(a);
        assert_eq!(meter.used(), 25);
        drop(b);
        assert_eq!(meter.used(), 0);
    }

    #[test]
    fn the_meter_books_with_the_budget_and_says_which_refused() {
        let budget = Arc::new(Counting::new(50));
        let meter = Meter::new(1000, budget.clone(), Reservation::Dependency);
        let a = meter.charge(40).unwrap();
        assert_eq!(budget.used(), 40);
        let e = meter.charge(20).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Budget(b) if b.what == Reservation::Dependency),
            "{e:?}"
        );
        assert_eq!(meter.used(), 40, "the budget's refusal is refunded");
        drop(a);
        assert_eq!((meter.used(), budget.used()), (0, 0));
    }

    #[test]
    fn absorbed_charges_return_when_the_owner_drops() {
        let m1 = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
        let m2 = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
        let mut c = m1.charge(10).unwrap();
        c.absorb(m1.charge(5).unwrap());
        c.absorb(m2.charge(7).unwrap());
        assert_eq!(c.bytes(), 22);
        assert_eq!((m1.used(), m2.used()), (15, 7));
        drop(c);
        assert_eq!((m1.used(), m2.used()), (0, 0));
    }

    #[test]
    fn a_metered_vec_charges_its_capacity_before_it_grows() {
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let mut v: MeteredVec<u64> = MeteredVec::new(&meter);
        for i in 0..1000 {
            v.push(i).unwrap();
        }
        let cap = u64::try_from(v.capacity()).unwrap();
        assert_eq!(
            meter.used(),
            heap_cost(cap * 8),
            "exactly the live allocation"
        );
        let (vec, charge) = v.into_parts();
        assert_eq!(vec.len(), 1000);
        drop(vec);
        drop(charge);
        assert_eq!(meter.used(), 0);
    }

    #[test]
    fn a_refused_doubling_retries_at_the_size_needed() {
        // 100 elements fit the meter; doubling 60 to 120 does not.
        let fits = heap_cost(100) + heap_cost(60);
        let meter = Meter::new(fits, Arc::new(Unbounded), Reservation::Input);
        let mut v: MeteredVec<u8> = MeteredVec::with_capacity(&meter, 60).unwrap();
        v.extend_from_slice(&[1; 60]).unwrap();
        v.extend_from_slice(&[2; 40]).unwrap();
        assert_eq!(v.capacity(), 100);
        assert_eq!(meter.used(), heap_cost(100));
    }

    #[test]
    fn heap_cost_covers_the_header_and_rounding() {
        assert_eq!(heap_cost(0), 0);
        assert_eq!(heap_cost(1), 32);
        assert_eq!(heap_cost(24), 32);
        assert_eq!(heap_cost(25), 48);
        assert_eq!(heap_cost(1000), 1008);
        assert_eq!(heap_cost(128 << 10), (128 << 10) + 4096);
        assert_eq!(heap_cost(u64::MAX), !4095);
        for n in 1..5000_u64 {
            assert!(heap_cost(n) >= n + 8, "{n}");
        }
    }

    #[test]
    fn heap_cost_is_glibcs_mmapped_chunk_at_every_residue() {
        // glibc: nb = (n + 23) & !15, and an mmapped chunk is
        // ALIGN_UP(nb + 8, 4096). Round 3's probe: n = 200,684 takes 204,800
        // bytes, a page more than n + 16 rounded up.
        assert_eq!(heap_cost(200_684), 204_800);
        assert_eq!(heap_cost(4_095_984), 4_100_096);
        // A request whose padded chunk reaches the threshold may be mmapped.
        assert_eq!(heap_cost(131_049), 135_168);
        for k in [32_u64, 33, 49, 1024, 1025] {
            for r in 0..=40 {
                let n = 4096 * k - r;
                let nb = (n + 23) & !15;
                // Only a padded chunk at the threshold or above may be mmapped.
                let worst = if nb < 128 << 10 {
                    nb
                } else {
                    (nb + 8 + 4095) & !4095
                };
                assert!(heap_cost(n) >= worst, "{n}");
            }
        }
    }

    #[test]
    fn a_reopen_has_its_own_work_and_shares_the_inputs_memory() {
        // Round 5: the Environment restarted the one meter's count when it
        // reopened an evicted dependency, so a nested bundle of the old open
        // that a resource read still held went on decoding on the restarted
        // allowance, and every reopen gave it a fresh one. A reopen is a new
        // meter on the same memory account; the old open keeps its count.
        let limits = Limits {
            max_input_memory: 1000,
            max_decode_work: 100,
            ..Limits::default()
        };
        let old = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Dependency);
        let held = old.charge(600).unwrap();
        old.count_decode(100).unwrap();
        assert!(old.work_used_up().is_some());
        let new = old.reopen();
        assert!(new.same_input(&old));
        assert_eq!(new.reservation(), Reservation::Dependency);
        new.count_decode(100).unwrap();
        let e = old.count_decode(1).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork),
            "the old open stays used up: {e:?}"
        );
        assert_eq!((old.decoded(), new.decoded()), (100, 100));
        // Both opens charge one memory bound.
        assert_eq!(new.used(), 600);
        let e = new.charge(401).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::InputMemory && l.value == 1001),
            "{e:?}"
        );
        let more = new.charge(400).unwrap();
        assert_eq!((old.used(), new.used()), (1000, 1000));
        drop((held, more));
        assert_eq!(old.used(), 0);
        let other = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        assert!(!other.same_input(&old));
    }

    #[test]
    fn work_done_is_counted_past_the_bound_and_refuses_what_follows() {
        let limits = Limits {
            max_decode_work: 100,
            ..Limits::default()
        };
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        meter.count_decode(40).unwrap();
        assert!(meter.work_used_up().is_none());
        // A computation that ended unkept was granted 90 bytes: counted as done.
        meter.add_work(90);
        assert_eq!(meter.decoded(), 130);
        let used_up = meter.work_used_up().unwrap();
        assert_eq!(
            (used_up.kind, used_up.value, used_up.max),
            (LimitKind::DecodeWork, 130, 100)
        );
        assert!(meter.count_decode(1).is_err());
        assert_eq!(meter.granted(), 0, "work is not memory");
        let c = meter.charge(7).unwrap();
        drop(c);
        assert_eq!(
            (meter.granted(), meter.used()),
            (7, 0),
            "grants are never returned"
        );
    }

    #[test]
    fn decode_work_is_counted_and_never_returned() {
        let limits = Limits {
            max_decode_work: 100,
            ..Limits::default()
        };
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        meter.count_decode(60).unwrap();
        meter.count_decode(40).unwrap();
        let e = meter.count_decode(1).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork && l.value == 101 && l.max == 100),
            "{e:?}"
        );
        assert_eq!(meter.decoded(), 100, "a refusal counts nothing");
        assert_eq!(meter.used(), 0, "work is not memory");
    }

    #[test]
    fn a_charge_keeps_one_record_per_meter_and_per_home() {
        // Round 3: under a Counting budget every growth left a Permit in its
        // owner's bookkeeping, uncharged.
        let budget = Arc::new(Counting::new(u64::MAX));
        let m1 = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let m2 = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut owner = Charge::zero(&m1);
        for i in 0..10_000_u64 {
            let mut c = Charge::zero(&m1);
            c.grow(1 + i % 7).unwrap();
            owner.absorb(c);
            owner.absorb(Charge::zero(&m1));
            owner.grow(3).unwrap();
            owner.absorb(m2.charge(2).unwrap());
        }
        assert_eq!(owner.records(), 1, "one record, for the other meter");
        assert_eq!(budget.used(), owner.bytes());
        let mut v: MeteredVec<u64> = MeteredVec::new(&m1);
        for i in 0..100_000 {
            v.push(i).unwrap();
        }
        assert_eq!(v.charge().records(), 0);
        drop((owner, v));
        assert_eq!((budget.used(), m1.used(), m2.used()), (0, 0, 0));
    }

    #[test]
    fn a_boxed_slice_is_charged_at_its_exact_size() {
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let mut v: MeteredVec<u32> = MeteredVec::new(&meter);
        for i in 0..1000 {
            v.push(i).unwrap();
        }
        assert!(v.capacity() > 1000);
        let (b, charge) = v.into_boxed_slice().unwrap();
        assert_eq!(b.len(), 1000);
        assert_eq!(meter.used(), heap_cost(4000));
        assert_eq!(charge.bytes(), heap_cost(4000));
        drop((b, charge));
        // A meter with no room for the exact copy refuses and keeps nothing.
        let tight = Meter::new(heap_cost(8 * 4), Arc::new(Unbounded), Reservation::Input);
        let mut v: MeteredVec<u32> = MeteredVec::with_capacity(&tight, 8).unwrap();
        v.push(1).unwrap();
        assert!(v.into_boxed_slice().is_err());
        assert_eq!(tight.used(), 0);
    }

    #[test]
    fn the_defaults() {
        let l = Limits::default();
        assert_eq!(l.max_input_memory, 8 << 30);
        assert_eq!(l.max_input_bytes, 4 << 30);
        assert_eq!(l.max_block_output, 1 << 30);
    }
}
