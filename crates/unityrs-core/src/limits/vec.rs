//! [`MeteredVec`], a `Vec` charged before it grows, and the growth every
//! charged buffer shares: geometric, charged first, retried at the exact
//! size ([`grow_charged`], [`grow_counted`]).

use std::fmt;
use std::mem::size_of;
use std::sync::Arc;

use super::{Charge, ChargeError, Meter, heap_cost};
use crate::budget::BudgetExceeded;
use crate::error::LimitKind;

/// `vec.try_reserve_exact(additional)`, after `meter` granted the `charged`
/// bytes it costs: an allocation the allocator refuses is a refusal of
/// those bytes for `meter`'s reservation, counted as a budget's refusal is
/// ([`super::refusal`]). Every allocation the core charges before it makes
/// it reports a failure this way.
pub(crate) fn reserve_exact<T>(
    vec: &mut Vec<T>,
    additional: usize,
    charged: u64,
    meter: &Meter,
) -> Result<(), BudgetExceeded> {
    vec.try_reserve_exact(additional)
        .map_err(|_| super::refusal(charged, meter.reservation()))
}

/// Gives back `vec`'s spare room, `charge` (which holds [`heap_cost`] of
/// its capacity, and may hold more) following it to [`heap_cost`] of its
/// length. Where glibc shrinks the block where it is (a heap chunk split in
/// place by `realloc`, a mapped one cut by `mremap`), no second block ever
/// exists, so nothing more is charged and the difference is returned after.
/// Where [`super::shrink_moves`] says the bytes move to a new allocation of
/// exactly their length (a block that may be mapped, given back below
/// 128 KiB), both blocks exist while they move, so the smaller one is
/// charged first, and a refusal, the meter's or the allocator's, leaves
/// `vec` and `charge` as they were (perf stage 2: the smaller block was
/// charged beside the old one either way, so a decoded run of 69.4 MB cut
/// from room of 128 MiB was charged both at once).
pub(crate) fn shrink_charged<T>(vec: &mut Vec<T>, charge: &mut Charge) -> Result<(), ChargeError> {
    let (len, cap) = (bytes_of::<T>(vec.len()), bytes_of::<T>(vec.capacity()));
    if len >= cap {
        return Ok(());
    }
    let (old, exact) = (heap_cost(cap), heap_cost(len));
    if super::shrink_moves(len, cap) {
        let smaller = charge.meter().charge(exact)?;
        let mut moved = Vec::new();
        if moved.try_reserve_exact(vec.len()).is_err() {
            // `smaller` drops here: nothing stays charged for it.
            let what = charge.meter().reservation();
            return Err(ChargeError::Budget(super::refusal(exact, what)));
        }
        moved.append(vec);
        *vec = moved;
        charge.release(old);
        charge.absorb(smaller);
    } else {
        vec.shrink_to_fit();
        charge.release(old - exact);
    }
    Ok(())
}

fn bytes_of<T>(n: usize) -> u64 {
    u64::try_from(n)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(size_of::<T>()).unwrap_or(u64::MAX))
}

/// Grows `vec`'s capacity to at least `need`, geometrically up to `max`
/// (never below `need`), charging the new allocation to `charge`'s meter
/// first. While the allocator moves the buffer both blocks exist, so both are
/// charged until it is done; then the old block's charge is returned. That
/// is a bound: exact where glibc copies the bytes to a new block, and an
/// over-count of the old block, for as long as the growth runs, wherever
/// glibc grows the block without a second one (plan section 3.15): a mapped
/// block, which it grows with `mremap`, its pages kept (every block of
/// 32 MiB or more, its largest `mmap` threshold, and a block of 128 KiB or
/// more while the threshold, which rises as mapped blocks are freed, is
/// still below it), and a heap chunk it extends in place into free space. A
/// refused doubling is asked for again with half the room past `need`, and
/// so on down to exactly `need` before it fails, so a meter that fits the
/// data is never refused by the growth policy, and a vector that meets its
/// bound grows a few dozen more times at most, each time by at least half of
/// what could still be granted ([`grow_counted`]).
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
///
/// The growth's ladder is `Text::reserve`'s (`unityrs-dump`, fix pass 15).
/// Fix pass 17: a refused doubling was asked for exactly `need`, so a vector
/// whose capacity lay between a third and a half of what was free grew by
/// one push at a time, each push past its room a refused request (a failed
/// `mmap` where the allocator is what refuses; review round 16, through the
/// walk's lists and the writer's [`crate::budget::DecodeBuf`]).
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
    let mut target = have.saturating_mul(2).min(max).max(need);
    loop {
        match grow_exact(vec, charge, target, work) {
            Ok(()) => return Ok(()),
            // The work bound refuses the exact size as well.
            Err(ChargeError::Limit(l)) if l.kind == LimitKind::DecodeWork => {
                return Err(ChargeError::Limit(l));
            }
            Err(_) if target > need => target = need + (target - need) / 2,
            Err(e) => return Err(e),
        }
    }
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
    if let Err(e) = reserve_exact(vec, additional, new, charge.meter()) {
        // `extra` drops here: nothing stays charged or counted for a failed
        // allocation.
        if work > 0 {
            charge.meter().uncount_decode(work);
        }
        return Err(ChargeError::Budget(e));
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

const _: () = crate::send_sync::<MeteredVec<u8>>();

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

    /// Room for `additional` more, growing geometrically but never past
    /// `most` elements in all (unless `additional` needs more); charged
    /// first.
    pub(crate) fn reserve_within(
        &mut self,
        additional: usize,
        most: usize,
    ) -> Result<(), ChargeError> {
        let need = self.vec.len().saturating_add(additional);
        grow_charged(&mut self.vec, &mut self.charge, need, most.max(need))
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

    /// Gives back any spare capacity, the charge following it: shrunk in
    /// place, its charge cut after (no second block exists, so nothing more
    /// is charged); or, for a block that may be mapped given back below
    /// 128 KiB ([`super::shrink_moves`]; review round 13), moved into an
    /// exact allocation charged before it is made (both blocks exist while
    /// it moves), so a refusal leaves the vector as it was and nothing
    /// allocated beyond what was charged.
    pub fn shrink_to_fit(&mut self) -> Result<(), ChargeError> {
        shrink_charged(&mut self.vec, &mut self.charge)
    }

    /// The elements as a boxed slice, and its charge, with no spare
    /// capacity ([`Self::shrink_to_fit`]).
    pub fn into_boxed_slice(mut self) -> Result<(Box<[T]>, Charge), ChargeError> {
        self.shrink_to_fit()?;
        Ok((self.vec.into_boxed_slice(), self.charge))
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
