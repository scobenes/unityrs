//! Registering a key (module docs, "Ranks", "Nested containers under
//! `LoadMode::Lazy`" and "Memory"): `register_cab`'s dict by rank, the lazy
//! slot a registration drops listed in the same step, or stood in for
//! (plan section 9, Q14), and what it dropped released after the lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(test)]
use super::Hook;
use super::{
    Cab, DepSlot, Dependency, Entry, LOAD_KEYS_HERE, ORDER_LEAF, PENDING_SHARE, Rank, Registry,
    RegistryInner, SHARE, SLOT_ARC, Slot, SlotKind, pending, reach, slot,
};
use crate::error::Error;
use crate::files::BundleFile;
use crate::limits::{Charge, Meter};
use crate::pystr::simplify_name;
use crate::raw::RawStr;

/// What a registration dropped (module docs): the slot and its charges,
/// and whether it was listed, with the steps the listing took.
struct Dropped {
    slot: Arc<Slot>,
    _charge: Charge,
    listed: Option<u64>,
}

/// The loads of a registration's rank: the value's (`value`) and the
/// position's (`position`, at most the value's). They differ only for the
/// entries of a container a slot stands in for (plan section 9, Q14),
/// which take their positions at the dropped slot's load, as its eager
/// load registered them, and their values at the standing slot's.
#[derive(Clone, Copy, Debug)]
pub(super) struct Loads {
    pub(super) value: u64,
    pub(super) position: u64,
}

impl Loads {
    /// A registration at `load` alone.
    pub(super) fn at(load: u64) -> Self {
        Self {
            value: load,
            position: load,
        }
    }
}

/// Which of two slots of one key stands in for the other (plan section 9,
/// Q14), told before the registration's write lock, with no lock held
/// (`reach::stands_in` may read a dependency's open handle), and applied
/// in the lock's step only when the key's value is still `standing`.
struct Stand {
    standing: Arc<Slot>,
    dropped: Arc<Slot>,
}

impl Stand {
    /// What `new` and the key's value `value` tell: the one of greater
    /// rank stands in for the other when that other is a lazy slot not yet
    /// reached and `reach::stands_in` says so.
    fn of(new: &Arc<Slot>, value: &Arc<Slot>) -> Option<Self> {
        let (standing, dropped) = match new.rank.cmp(&value.rank) {
            std::cmp::Ordering::Greater => (new, value),
            std::cmp::Ordering::Less => (value, new),
            std::cmp::Ordering::Equal => return None,
        };
        (dropped.unreached() && reach::stands_in(standing, dropped)).then(|| Self {
            standing: standing.clone(),
            dropped: dropped.clone(),
        })
    }
}

/// Who makes a registration: a load (a new key counts for the container
/// pass's `RuntimeError`), or the reach of a nested container.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    Load,
    Reach,
}

impl Registry {
    /// A new load's sequence number: its registrations come after every
    /// earlier load's (module docs).
    pub(crate) fn next_load(&self) -> u64 {
        self.loads.fetch_add(1, Ordering::Relaxed)
    }

    /// `register_cab(name, cab)` for a file the caller loaded, as a load of
    /// its own, charged to `meter` (module docs).
    pub fn register(&self, name: &RawStr, cab: Cab, meter: &Arc<Meter>) -> Result<(), Error> {
        let load = self.next_load();
        self.register_ready(name, cab, load, &[], meter)
    }

    /// Registers entry `dir_index` of a bundle the caller loaded `Lazy`, as a
    /// load of its own, to parse on first use; charged to `meter`.
    pub fn register_lazy(
        &self,
        name: &RawStr,
        bundle: Arc<BundleFile>,
        dir_index: usize,
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let load = self.next_load();
        self.register_entry(name, bundle, dir_index, load, &[dir_index], meter)
    }

    /// Registers a caller's CAB of load `load` at `path`.
    pub(crate) fn register_ready(
        &self,
        name: &RawStr,
        cab: Cab,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        self.insert(
            name,
            SlotKind::Ready(cab),
            Loads::at(load),
            path,
            meter,
            By::Load,
            false,
        )
    }

    /// Registers entry `dir_index` of a caller's `Lazy` bundle, of load
    /// `load` at `path`, unread (a lazy slot, module docs).
    pub(crate) fn register_entry(
        &self,
        name: &RawStr,
        bundle: Arc<BundleFile>,
        dir_index: usize,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let kind = SlotKind::Entry { bundle, dir_index };
        self.insert(name, kind, Loads::at(load), path, meter, By::Load, true)
    }

    /// Registers a dependency's CAB of load `load` at `path` (module docs),
    /// charged to `meter`, the dependency's.
    pub(crate) fn register_dependency(
        &self,
        name: &RawStr,
        dep: &Arc<Dependency>,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let kind = SlotKind::Dependency {
            dep: DepSlot::new(dep),
        };
        self.insert(name, kind, Loads::at(load), path, meter, By::Load, false)
    }

    /// Registers entry `path` of a dependency's `Lazy` bundle, of load
    /// `load`, unread (a lazy slot, module docs), charged to `meter`, the
    /// dependency's.
    pub(crate) fn register_lazy_dependency(
        &self,
        name: &RawStr,
        dep: &Arc<Dependency>,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let kind = SlotKind::Dependency {
            dep: DepSlot::new(dep),
        };
        self.insert(name, kind, Loads::at(load), path, meter, By::Load, true)
    }

    /// Registers `kind` at rank `(loads.value, path)` under
    /// `simplify_name(name)` with dict semantics by rank (module docs), as
    /// a registration at `(loads.position, path)` followed by one at its
    /// own rank would: a new key takes its place in the order at the
    /// position and pays its share of the tables and a copy of itself; a
    /// known key takes the slot only if its rank is greater than its
    /// slot's, moves to the position if that is less than its own, and pays
    /// only for the slot (and its new position's path when it moves).
    /// Charged before anything is allocated; a refusal registers nothing. A
    /// lazy slot the registration drops unreached (the value it replaces,
    /// or its own slot when that ranks below the value) is listed in the
    /// same step, unless the slot that stays stands in for it (plan section
    /// 9, Q14: told before the lock, [`Stand`]), which then takes its
    /// position; a load's registration reaches every listed slot before it
    /// returns ([`Self::settle`], module docs), and a change is noted for
    /// the running container preload walks ([`super::watch`]).
    // The registration's parts, each one of the module docs' terms.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn insert(
        &self,
        name: &RawStr,
        kind: SlotKind,
        loads: Loads,
        path: &[usize],
        meter: &Arc<Meter>,
        by: By,
        lazy: bool,
    ) -> Result<(), Error> {
        // UnityPy environment.py:252-264
        let lowered = simplify_name(name, meter)?;
        let rank = Rank {
            load: loads.value,
            path: Arc::from(path),
        };
        // The position shares the value's path, which `first_charge` pays
        // for while the key keeps it.
        let position = Rank {
            load: loads.position.min(loads.value),
            path: rank.path.clone(),
        };
        let slot = Self::new_slot(kind, rank.clone(), lazy, meter)?;
        let (known, first, value) = {
            let inner = self.read();
            let value = inner.slots.get(lowered.as_bytes()).map(|e| e.slot.clone());
            (value.is_some(), inner.order.is_empty(), value)
        };
        let stand = value.as_ref().and_then(|v| Stand::of(&slot, v));
        drop(value);
        // A new key's own copy and its share of the tables, made before the
        // lock: the key holds no input's buffer (module docs); the first key
        // also pays for the B-tree's root. A known key may move to this
        // rank: its path is charged now and returned if it does not.
        let fresh = if known {
            None
        } else {
            let share = meter.charge(if first { SHARE + ORDER_LEAF } else { SHARE })?;
            Some((RawStr::charged(lowered.as_bytes(), meter)?, share))
        };
        let first_charge = meter.charge(Rank::heap(path.len()))?;
        let (dropped, changed, new_key) = {
            let mut inner = self.write();
            let RegistryInner {
                order,
                slots,
                pending,
            } = &mut *inner;
            if let Some(e) = slots.get_mut(lowered.as_bytes()) {
                let moves = position < e.first;
                let charge = if moves {
                    if let Some(key) = order.remove(&e.first) {
                        order.insert(position.clone(), key);
                    }
                    e.first = position;
                    std::mem::replace(&mut e.first_charge, first_charge)
                } else {
                    first_charge
                };
                let replaces = rank > e.slot.rank;
                // Equal ranks are one entry registered again: nothing of it
                // is dropped.
                let distinct = rank != e.slot.rank;
                let slot = if replaces {
                    std::mem::replace(&mut e.slot, slot)
                } else {
                    slot
                };
                let listed = (distinct
                    && slot.unreached()
                    && !Self::stood_in(stand.as_ref(), &e.slot, &slot))
                .then(|| {
                    let (_, steps) = pending.push(slot.clone());
                    self.store_lens(pending);
                    steps
                });
                let dropped = Dropped {
                    slot,
                    _charge: charge,
                    listed,
                };
                (Some(dropped), moves || replaces, false)
            } else if let Some((key, share)) = fresh {
                order.insert(position.clone(), key.clone());
                slots.reserve(1);
                slots.insert(
                    key,
                    Entry {
                        slot,
                        first: position,
                        _share: share,
                        first_charge,
                    },
                );
                (None, true, true)
            } else {
                // The key went away between the check and the lock: never,
                // since nothing removes one; register it on the next call.
                return Err(Error::Malformed {
                    what: "registry",
                    detail: "a key vanished".into(),
                });
            }
        };
        if changed {
            #[cfg(test)]
            self.hook(Hook::Published);
            self.watches.notify(lowered.as_bytes());
        }
        // A reach registers what an eager load registered at load,
        // before any container pass: only a load's change starts a new one.
        if changed && by == By::Load {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        if new_key && by == By::Load {
            LOAD_KEYS_HERE.with(|c| c.set(c.get().saturating_add(1)));
        }
        drop(stand);
        self.release(dropped);
        if by == By::Load {
            // A refusal leaves it for the next call (`settle`).
            self.settle().ok();
        }
        Ok(())
    }

    /// Whether `standing`, the key's value after a registration, stands in
    /// for `dropped`, the slot it dropped, as `stand` told before the lock
    /// (plan section 9, Q14); under the write lock. When it does, the
    /// standing slot takes the dropped one's position
    /// ([`Slot::position`]), and the dropped slot is not listed. A standing
    /// slot that is the new registration's own is not yet published, so no
    /// reach can have closed its position; one that was already the key's
    /// value may be reaching now, and stands in only while its position is
    /// not closed ([`Slot::stand_for`]): a reach closes it only at the
    /// position it registered at, so a lowering it meets first is
    /// registered before the slot is marked reached, and a registration
    /// that meets the close lists the dropped slot, reached as any other
    /// drop (`reach.rs`'s module docs).
    fn stood_in(stand: Option<&Stand>, standing: &Arc<Slot>, dropped: &Arc<Slot>) -> bool {
        let Some(stand) = stand else {
            return false;
        };
        if !(Arc::ptr_eq(&stand.standing, standing) && Arc::ptr_eq(&stand.dropped, dropped)) {
            return false;
        }
        standing.stand_for(dropped.position())
    }

    /// Publishes the lists' lengths (under the write lock).
    pub(super) fn store_lens(&self, pending: &pending::Pending) {
        self.pending_len
            .store(pending.len(pending::List::Listed), Ordering::Release);
        self.evicted_len
            .store(pending.len(pending::List::Evicted), Ordering::Release);
    }

    /// A slot of `kind` at `rank`, charged to `meter` (its `Arc`, its
    /// rank's path, and a lazy slot's share of the list of dropped slots).
    fn new_slot(
        kind: SlotKind,
        rank: Rank,
        lazy: bool,
        meter: &Arc<Meter>,
    ) -> Result<Arc<Slot>, Error> {
        let mut charge = Charge::zero(meter);
        charge.grow(SLOT_ARC)?;
        charge.grow(Rank::heap(rank.path().len()))?;
        if lazy {
            charge.grow(PENDING_SHARE)?;
        }
        Ok(Arc::new(Slot {
            kind,
            rank,
            lazy,
            reached: AtomicBool::new(false),
            stands_for: AtomicU64::new(slot::NONE),
            counted: AtomicBool::new(false),
            walked: AtomicBool::new(false),
            charge,
        }))
    }

    /// Drops what a registration replaced or did not use (and its charges),
    /// after the lock, and counts the steps a listing took as the dropped
    /// slot's input's work (`pending.rs`). A lazy slot not yet reached was
    /// listed with the change unless the key's value stands in for it; a
    /// load's registration then reaches every listed slot before it
    /// returns, and a reach's registrations leave theirs to the `settle`
    /// that runs the reach (`reach.rs`).
    // `self` runs the test's hook.
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn release(&self, dropped: Option<Dropped>) {
        if let Some(d) = &dropped
            && let Some(steps) = d.listed
        {
            d.slot
                .input_meter()
                .add_work(steps.saturating_mul(pending::STEP));
            #[cfg(test)]
            self.hook(Hook::Deferred);
        }
        drop(dropped);
    }
}
