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

/// Below this, glibc's malloc serves an allocation from its heap; at or
/// above it, possibly from `mmap` (its threshold starts here and can rise).
const MMAP_THRESHOLD: u64 = 128 << 10;

/// What one heap allocation of `bytes` costs, which is what a charge for it
/// is: glibc's chunk on x86-64, its 8-byte header added and rounded to 16
/// with a 32-byte minimum, and for a large allocation (one that may be
/// `mmap`ped) the 16-byte header added and rounded to a 4 KiB page. So a
/// charge covers the allocator's rounding too, and many small allocations
/// cannot hold more than they were charged. 0 for no allocation.
#[must_use]
pub const fn heap_cost(bytes: u64) -> u64 {
    if bytes == 0 {
        0
    } else if bytes < MMAP_THRESHOLD {
        let chunk = bytes.saturating_add(8 + 15) & !15;
        if chunk < 32 { 32 } else { chunk }
    } else {
        bytes.saturating_add(16 + 4095) & !4095
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

/// What one input holds, against [`Limits::max_input_memory`] (module docs).
/// Thread-safe: a lazily read bundle's entries charge one meter from any
/// thread. Shared as `Arc<Meter>`; every [`Charge`] keeps it alive.
pub struct Meter {
    used: AtomicU64,
    limit: u64,
    budget: Arc<dyn MemoryBudget>,
    what: Reservation,
}

impl fmt::Debug for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Meter")
            .field("used", &self.used())
            .field("limit", &self.limit)
            .field("what", &self.what)
            .finish_non_exhaustive()
    }
}

impl Meter {
    /// A meter of `limit` bytes whose charges are booked with `budget` as `what`.
    #[must_use]
    pub fn new(limit: u64, budget: Arc<dyn MemoryBudget>, what: Reservation) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(0),
            limit,
            budget,
            what,
        })
    }

    /// The meter of one input loaded with `limits` and `budget`.
    #[must_use]
    pub fn for_input(
        limits: &Limits,
        budget: Arc<dyn MemoryBudget>,
        what: Reservation,
    ) -> Arc<Self> {
        Self::new(limits.max_input_memory, budget, what)
    }

    /// A meter with no bound and no budget, for data that is not an input's:
    /// a TPK's trees, a caller's `nodes=` rows, tests.
    #[must_use]
    pub fn unlimited() -> Arc<Self> {
        Self::new(u64::MAX, Arc::new(Unbounded), Reservation::Input)
    }

    /// Bytes charged and not yet returned.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// The bound.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// How the budget books this meter's charges.
    #[must_use]
    pub fn reservation(&self) -> Reservation {
        self.what
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
        let limit = self.limit;
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|t| *t <= limit)
            })
            .map_err(|used| LimitError {
                kind: LimitKind::InputMemory,
                value: used.saturating_add(bytes),
                max: limit,
            })?;
        self.budget.try_reserve(bytes, self.what).map_err(|e| {
            self.refund(bytes);
            ChargeError::Budget(e)
        })
    }

    fn refund(&self, bytes: u64) {
        // Saturating: a refund never takes the count below zero.
        let _ = self
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                Some(used.saturating_sub(bytes))
            });
    }
}

/// Bytes charged to a [`Meter`] (and booked with its budget), returned when
/// the charge drops. It rides inside whatever it paid for.
#[must_use = "a charge returns its bytes when dropped"]
pub struct Charge {
    meter: Arc<Meter>,
    bytes: u64,
    permit: Permit,
    /// Absorbed charges of other meters.
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

    /// Takes `other` in: its bytes are returned when this charge drops.
    pub fn absorb(&mut self, mut other: Charge) {
        self.more.append(&mut other.more);
        if Arc::ptr_eq(&self.meter, &other.meter) {
            self.bytes = self.bytes.saturating_add(other.bytes);
            other.bytes = 0;
            self.permit
                .absorb(std::mem::replace(&mut other.permit, Permit::detached(0)));
        } else if other.bytes > 0 {
            self.more.push(other);
        }
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
    let have = vec.capacity();
    if need <= have {
        return Ok(());
    }
    let doubled = have.saturating_mul(2).min(max).max(need);
    if doubled > need && grow_exact(vec, charge, doubled).is_ok() {
        return Ok(());
    }
    grow_exact(vec, charge, need)
}

fn grow_exact<T>(vec: &mut Vec<T>, charge: &mut Charge, cap: usize) -> Result<(), ChargeError> {
    let old = heap_cost(bytes_of::<T>(vec.capacity()));
    let new = heap_cost(bytes_of::<T>(cap));
    let mut extra = charge.meter().charge(new)?;
    let additional = cap.saturating_sub(vec.len());
    if vec.try_reserve_exact(additional).is_err() {
        // `extra` drops here: nothing stays charged for a failed allocation.
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

    /// Cuts to `len` elements; the allocation and its charge stay.
    pub fn truncate(&mut self, len: usize) {
        self.vec.truncate(len);
    }

    /// The vector and its charge, which must live as long as the vector's
    /// allocation does.
    pub fn into_parts(self) -> (Vec<T>, Charge) {
        (self.vec, self.charge)
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

    use super::{ChargeError, Limits, Meter, MeteredVec, heap_cost};
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
    fn the_defaults() {
        let l = Limits::default();
        assert_eq!(l.max_input_memory, 8 << 30);
        assert_eq!(l.max_input_bytes, 4 << 30);
        assert_eq!(l.max_block_output, 1 << 30);
    }
}
