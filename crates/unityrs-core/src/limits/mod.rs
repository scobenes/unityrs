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
//!
//! [`MemoryBudget`]: crate::budget::MemoryBudget
//! [`Reservation`]: crate::budget::Reservation

use std::fmt;
use std::mem::size_of;
use std::sync::Arc;

use crate::budget::BudgetExceeded;
use crate::error::{LimitError, LimitKind};

mod meter;

pub(crate) use meter::GrantScope;
pub use meter::{Charge, Meter};

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
    /// block decode writes, kept or not (a first decode, a measuring decode
    /// whose output is dropped, a re-decode of a shared block, a decode that
    /// fails after it wrote), whether or not its buffer had to grow for it,
    /// and every byte the meter granted to a lazy computation, on the
    /// computation's own thread, that the meter itself then refused
    /// ([`LimitKind::InputMemory`]: a parse or compile that alone needs more
    /// than the meter, which the next ask runs again:
    /// [`crate::once::get_or_publish_result`]). The meter bounds what is
    /// held; this bounds what is done, so an input cannot make its readers
    /// decode or recompute without end while the meter stays empty
    /// (thousands of blocks that each decode to the block cap, measured and
    /// dropped by a lazy read of the last entry; one type whose compile alone
    /// needs more than the meter, asked by every object of the type). A
    /// write the meter or the budget refuses decodes nothing and counts
    /// nothing, and a lazy computation the budget, an I/O failure or a
    /// provider refused counts nothing either (those depend on what else is
    /// held or on caller code, and the engine bounds its retries), so
    /// contention never uses the allowance up; a write is counted atomically
    /// (between a growth's charge and its allocation when it grows), so
    /// decodes running at once never pass the bound
    /// ([`Meter::count_decode`]). It is counted
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
    /// Runtime nesting while reading or writing a value, `ReferencedObject`
    /// data included: the frames each reader and the writer recurse through
    /// (plan section 3.10, "Bounds"). UnityPyBoost's are its C recursion,
    /// which has no bound of its own; the pure-Python reader's and the
    /// writer's are the Python frames UnityPy's own pushes (`read_value`,
    /// `read_value_array`, each comprehension and `lambda`), so every value
    /// UnityPy refuses with `RecursionError` (Python's 1,000 frames) passes
    /// this first. What it costs in stack: [`Self::stack_needed`]. Default 512.
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

impl Limits {
    /// The stack a thread needs to read or write one object under these
    /// limits, from the frame that calls the reader or the writer: they
    /// recurse once per level they count against
    /// [`Self::max_value_depth`], and nothing else an object's bytes drive
    /// recurses deeper (a typetree compiles without recursion; a value is
    /// dropped one level per reader level). It grows with `max_value_depth`,
    /// and a debug build's frames are several times a release build's.
    ///
    /// At the defaults: 1.75 MiB in a release build, 9 MiB in a debug one.
    /// `tests/walk_stack.rs` reads, writes and drops the deepest object of
    /// every shape on a thread this size, and its `stack_measured` prints
    /// the least each needs (at most 0.96 MiB release, 6.7 MiB debug, with
    /// toolchain 1.99.1). Rust's spawned threads get 2 MiB unless told
    /// otherwise, and a main thread usually 8 MiB, so a debug build reads
    /// untrusted input on a thread built with
    /// `std::thread::Builder::stack_size(limits.stack_needed())`.
    #[must_use]
    pub fn stack_needed(&self) -> usize {
        let (per_level, base): (usize, usize) = if cfg!(debug_assertions) {
            (16 << 10, 1 << 20)
        } else {
            (3 << 10, 256 << 10)
        };
        usize::try_from(self.max_value_depth)
            .unwrap_or(usize::MAX)
            .saturating_mul(per_level)
            .saturating_add(base)
    }
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
/// ([`Meter::count_decode`]) whether or not `vec` grows: with no growth,
/// at once; with one, once the growth's memory is granted and before it is
/// allocated, so a growth the meter or the budget refuses counts nothing,
/// and one the work bound refuses returns its charge and allocates nothing.
/// The check and the count are one atomic step, so writes on several
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
        // Round 6: the work of a write into room already there went
        // uncounted (a pre-sized or reused buffer decoded without bound).
        if work > 0 {
            charge.meter().count_decode(work)?;
        }
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
mod tests;
