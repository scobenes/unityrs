//! The cutter's tables, each charged to its meter before it grows, at
//! the size of what it allocates (`docs/plans/batch.md` 2.6 steps 5 and
//! 10; review round 2's S3): [`Set`], a hash set charged by its buckets;
//! [`with_capacity`] and [`push`], a [`MeteredVec`]'s growth, its refusal
//! as the refusal probe's class ([`refused`]); [`slots`] and [`string`],
//! a table's elements and a string at their size.
//!
//! A vector is the core's [`MeteredVec`]: its capacity charged at
//! [`heap_cost`] before each growth, the old block's charge kept until the
//! new one is made. Each table's charge drops with it.

use std::borrow::Borrow;
use std::collections::HashSet;
use std::hash::Hash;
use std::mem::size_of;
use std::sync::Arc;

use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};

use super::classify::{self, Class, Reason};

/// `n` elements of `T`, as a charge's bytes.
#[must_use]
pub fn slots<T>(n: usize) -> u64 {
    u64::try_from(size_of::<T>())
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(n).unwrap_or(u64::MAX))
}

/// A string of `len` bytes, as a charge's bytes.
#[must_use]
pub fn string(len: usize) -> u64 {
    heap_cost(u64::try_from(len).unwrap_or(u64::MAX))
}

/// A charge's refusal as its class: the meter's or the budget's refusal,
/// or an allocator's, which the core reports as a budget's (the refusal
/// probe tells them apart, 2.7).
pub fn refused(e: &ChargeError) -> Class {
    classify::charge(e)
}

/// The allocator's refusal of an allocation the charge granted: the stop
/// `host_memory` (2.7).
#[must_use]
pub fn host_memory() -> Class {
    Class::Run(Reason::plain("host_memory"))
}

/// An empty vector on `meter` with room for exactly `n`, charged first.
pub fn with_capacity<T>(meter: &Arc<Meter>, n: usize) -> Result<MeteredVec<T>, Class> {
    MeteredVec::with_capacity(meter, n).map_err(|e| refused(&e))
}

/// Pushes `value` onto `v`, its growth charged first.
pub fn push<T>(v: &mut MeteredVec<T>, value: T) -> Result<(), Class> {
    v.push(value).map_err(|e| refused(&e))
}

/// The buckets a hash table of `capacity` takes: at most the next power of
/// two of `capacity x 8 / 7`, and 4 at the least (hashbrown's load factor,
/// which std's `HashSet` uses: an upper bound of its layout).
fn buckets(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    let c = u64::try_from(capacity).unwrap_or(u64::MAX);
    let wanted = c.saturating_mul(8).div_ceil(7);
    wanted
        .checked_next_power_of_two()
        .unwrap_or(u64::MAX)
        .max(4)
}

/// What a hash table of `buckets` buckets of `slot` bytes allocates: the
/// slots, padded to the control bytes' alignment (a group's width), then a
/// control byte per bucket and a group of control bytes past them.
fn table_cost(buckets: u64, slot: usize) -> u64 {
    if buckets == 0 {
        return 0;
    }
    let slot = u64::try_from(slot).unwrap_or(u64::MAX);
    let slots = buckets
        .saturating_mul(slot)
        .div_ceil(GROUP)
        .saturating_mul(GROUP);
    heap_cost(slots.saturating_add(buckets).saturating_add(GROUP))
}

/// The control bytes past a table's buckets (hashbrown's widest group).
const GROUP: u64 = 16;

/// A hash set whose table is charged by its buckets: before it grows, the
/// new table (beside the old one, which the move keeps until it is done);
/// after, the table it has. An element's own heap is its holder's to
/// charge.
#[derive(Debug)]
pub struct Set<T> {
    set: HashSet<T>,
    charge: Charge,
}

impl<T: Hash + Eq> Set<T> {
    /// An empty set charging `meter`.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            set: HashSet::new(),
            charge: Charge::zero(meter),
        }
    }

    /// Whether `value` is in the set.
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.set.contains(value)
    }

    /// How many it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Room for one more, its growth charged first: a refused charge or
    /// allocation leaves the set as it was.
    pub fn reserve_one(&mut self) -> Result<(), Class> {
        let have = self.set.capacity();
        if self.set.len() < have {
            return Ok(());
        }
        let slot = size_of::<T>();
        let old = table_cost(buckets(have), slot);
        let new = table_cost(buckets(have.saturating_add(1)), slot);
        let meter = self.charge.meter().clone();
        let mut extra = meter.charge(new).map_err(|e| refused(&e))?;
        self.set.try_reserve(1).map_err(|_| host_memory())?;
        let got = table_cost(buckets(self.set.capacity()), slot);
        if got > new {
            extra.grow(got - new).map_err(|e| refused(&e))?;
        } else {
            extra.release(new - got);
        }
        self.charge.release(old);
        self.charge.absorb(extra);
        Ok(())
    }

    /// Inserts `value`: false when it was there; its growth charged first.
    pub fn insert(&mut self, value: T) -> Result<bool, Class> {
        if self.set.contains(&value) {
            return Ok(false);
        }
        self.reserve_one()?;
        Ok(self.set.insert(value))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::limits::Meter;

    use super::{Set, buckets};

    #[test]
    fn a_sets_charge_follows_its_buckets() {
        assert_eq!(buckets(0), 0);
        assert_eq!(buckets(3), 4);
        assert_eq!(buckets(7), 8);
        assert_eq!(buckets(14), 16);
        assert_eq!(buckets(15), 32);
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let mut set = Set::new(&meter);
        for i in 0..1000u64 {
            assert!(set.insert(i).unwrap());
            let cap = set.set.capacity();
            assert!(buckets(cap) * 9 + 16 <= meter.used(), "{i}: {cap}");
        }
        assert!(!set.insert(5).unwrap());
        drop(set);
        assert_eq!(meter.used(), 0);
    }

    #[test]
    fn a_refused_growth_leaves_the_set_as_it_was() {
        let meter = Meter::new(200, Arc::new(Unbounded), Reservation::Input);
        let mut set = Set::new(&meter);
        let mut n = 0u64;
        while set.insert(n).is_ok() {
            n += 1;
        }
        assert_eq!(set.len(), usize::try_from(n).unwrap());
        assert!(meter.used() <= 200);
    }
}
