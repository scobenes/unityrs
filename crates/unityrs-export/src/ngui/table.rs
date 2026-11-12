//! The cutter's tables, each charged to its meter before it grows, at
//! the size of what it allocates (`docs/plans/batch.md` 2.6 steps 5 and
//! 10; review round 2's S3): [`Set`] and [`Map`], a hash set and a hash
//! map charged by their buckets;
//! [`with_capacity`] and [`push`], a [`MeteredVec`]'s growth, its refusal
//! as the refusal probe's class ([`refused`]); [`slots`] and [`string`],
//! a table's elements and a string at their size.
//!
//! A vector is the core's [`MeteredVec`]: its capacity charged at
//! [`heap_cost`] before each growth, the old block's charge kept until the
//! new one is made. Each table's charge drops with it.

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
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

/// A copy of `bytes`, its room reserved exactly (an allocator's refusal
/// `host_memory`); its charge is its caller's, taken before
/// ([`Map::insert_held`]).
pub fn copied(bytes: &[u8]) -> Result<Vec<u8>, Class> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| host_memory())?;
    copy.extend_from_slice(bytes);
    Ok(copy)
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

/// A table of `len` of `have` slots of `slot` bytes made room for one
/// more by `reserve` (which gives the new capacity), the new table charged
/// to `charge` first, beside the old one, and the old one's charge
/// returned once it is gone: a refused charge or allocation leaves it as
/// it was.
fn grow_one<E>(
    charge: &mut Charge,
    len: usize,
    have: usize,
    slot: usize,
    reserve: impl FnOnce() -> Result<usize, E>,
) -> Result<(), Class> {
    if len < have {
        return Ok(());
    }
    let old = table_cost(buckets(have), slot);
    let new = table_cost(buckets(have.saturating_add(1)), slot);
    let meter = charge.meter().clone();
    let mut extra = meter.charge(new).map_err(|e| refused(&e))?;
    let got = table_cost(buckets(reserve().map_err(|_| host_memory())?), slot);
    if got > new {
        extra.grow(got - new).map_err(|e| refused(&e))?;
    } else {
        extra.release(new - got);
    }
    charge.release(old);
    charge.absorb(extra);
    Ok(())
}

/// A hash map whose table is charged by its buckets, as [`Set`]'s is: a
/// key's or a value's own heap is its holder's to charge.
#[derive(Debug)]
pub struct Map<K, V> {
    map: HashMap<K, V, State>,
    charge: Charge,
}

/// The maps' hasher: the standard library's, and under the `test-counters`
/// feature the same with every byte it hashes counted
/// (`crate::testing::Counting`), so a test holds the hashing done to the
/// work counted (B3's review round 5, Q3).
#[cfg(not(feature = "test-counters"))]
type State = std::hash::RandomState;
#[cfg(feature = "test-counters")]
type State = crate::testing::Counting;

impl<K: Hash + Eq, V> Map<K, V> {
    /// An empty map charging `meter`.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            map: HashMap::with_hasher(State::default()),
            charge: Charge::zero(meter),
        }
    }

    /// The value under `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(key)
    }

    /// The value under `key`, to change.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get_mut(key)
    }

    /// Puts `value` under `key`, its growth charged first (a refused charge
    /// or allocation leaves the map as it was); a value already there is
    /// replaced.
    pub fn insert(&mut self, key: K, value: V) -> Result<(), Class> {
        if let Some(v) = self.map.get_mut(&key) {
            *v = value;
            return Ok(());
        }
        let map = &mut self.map;
        let (len, have) = (map.len(), map.capacity());
        grow_one(&mut self.charge, len, have, size_of::<(K, V)>(), || {
            map.try_reserve(1).map(|()| map.capacity())
        })?;
        self.map.insert(key, value);
        Ok(())
    }

    /// [`Self::insert`] of an entry that holds `cost` bytes of its own (a
    /// key's copy, a pin), charged to a charge of its own first, its key
    /// made (`key`, which allocates the copy) only once that charge is
    /// granted, and the charge kept in `held` only once the insert
    /// succeeds: a refused charge makes nothing, and a refused charge or
    /// insert keeps nothing (B3's review rounds 3 and 4, S7 and R2).
    pub fn insert_held(
        &mut self,
        held: &mut Charge,
        cost: u64,
        key: impl FnOnce() -> Result<K, Class>,
        value: V,
    ) -> Result<(), Class> {
        let charge = held.meter().charge(cost).map_err(|e| refused(&e))?;
        self.insert(key()?, value)?;
        held.absorb(charge);
        Ok(())
    }
}

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
        let set = &mut self.set;
        let (len, have) = (set.len(), set.capacity());
        grow_one(&mut self.charge, len, have, size_of::<T>(), || {
            set.try_reserve(1).map(|()| set.capacity())
        })
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
    use unityrs_core::limits::{Charge, Meter};

    use super::{Map, Set, buckets};

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
    fn a_maps_charge_follows_its_buckets() {
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let mut map = Map::new(&meter);
        for i in 0..1000u64 {
            map.insert(i, [i; 3]).unwrap();
            let cap = map.map.capacity();
            assert!(buckets(cap) * (32 + 1) + 16 <= meter.used(), "{i}: {cap}");
        }
        let used = meter.used();
        map.insert(5, [0; 3]).unwrap();
        assert_eq!((meter.used(), map.get(&5)), (used, Some(&[0; 3])));
        assert_eq!(map.map.len(), 1000);
        drop(map);
        assert_eq!(meter.used(), 0);
        let small = Meter::new(300, Arc::new(Unbounded), Reservation::Input);
        let mut map = Map::new(&small);
        let mut n = 0u64;
        while map.insert(n, n).is_ok() {
            n += 1;
        }
        assert_eq!(map.map.len(), usize::try_from(n).unwrap());
        assert!(small.used() <= 300);
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

    /// A budget that refuses its `at`-th reservation, counting from 0.
    #[derive(Debug)]
    struct RefuseAt {
        asked: AtomicUsize,
        at: usize,
    }

    impl MemoryBudget for RefuseAt {
        fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
            if self.asked.fetch_add(1, Ordering::AcqRel) == self.at {
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
            Unbounded.try_reserve(bytes, what)
        }
    }

    #[test]
    fn a_held_charge_is_kept_only_once_its_insert_succeeds() {
        // B3's review round 3, S7: the entry's own charge is the first
        // reservation, the table's growth the second; refusing the growth
        // keeps neither, and the retry's usage is one success's.
        let once = || {
            let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
            let mut held = Charge::zero(&meter);
            let mut map = Map::new(&meter);
            map.insert_held(&mut held, 40, || Ok(b"name".to_vec()), 1u32)
                .unwrap();
            (meter.used(), held.bytes())
        };
        let budget = Arc::new(RefuseAt {
            asked: AtomicUsize::new(0),
            at: 1,
        });
        let meter = Meter::new(u64::MAX, budget, Reservation::Input);
        let mut held = Charge::zero(&meter);
        let mut map = Map::new(&meter);
        assert!(
            map.insert_held(&mut held, 40, || Ok(b"name".to_vec()), 1u32)
                .is_err()
        );
        assert_eq!(
            (meter.used(), held.bytes(), map.get(&b"name"[..])),
            (0, 0, None)
        );
        map.insert_held(&mut held, 40, || Ok(b"name".to_vec()), 1u32)
            .unwrap();
        assert_eq!((meter.used(), held.bytes()), once());
        assert_eq!(map.get(&b"name"[..]), Some(&1));
        // A refused entry charge keeps nothing either.
        let budget = Arc::new(RefuseAt {
            asked: AtomicUsize::new(0),
            at: 0,
        });
        let meter = Meter::new(u64::MAX, budget, Reservation::Input);
        let mut held = Charge::zero(&meter);
        let mut map = Map::new(&meter);
        // And makes no key (round 4, R2): nothing is allocated for it.
        let made = std::cell::Cell::new(false);
        let key = || {
            made.set(true);
            Ok(b"name".to_vec())
        };
        assert!(map.insert_held(&mut held, 40, key, 1u32).is_err());
        assert!(!made.get(), "the key's copy was made before its charge");
        assert_eq!(meter.used(), 0);
        map.insert_held(&mut held, 40, || Ok(b"name".to_vec()), 1u32)
            .unwrap();
        assert_eq!((meter.used(), held.bytes()), once());
    }
}
