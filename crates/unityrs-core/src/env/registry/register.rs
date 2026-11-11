// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! Registering a key (module docs, "Ranks", "Nested containers under
//! `LoadMode::Lazy`" and "Memory"): `register_cab`'s dict by rank, the lazy
//! slot a registration drops listed in the same step, or stood in for
//! (plan section 9, Q14), and what it dropped released after the lock.
//!
//! A registration is charged first ([`Registry::prepare`]: every charge it
//! makes) and published after ([`Registry::apply`]: nothing in it can fail
//! the registration; the `settle` a load's runs reaches the slots it
//! listed, and a refusal there leaves them listed for a later call, as
//! every `settle` does). A load registers each key in turn, each charged
//! and then published; a dependency's load under `LoadMode::Lazy` charges
//! every one of its registrations before it publishes the first
//! ([`Stage`], module docs, "A failed dependency load").

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
use crate::limits::{Charge, Meter, MeteredVec};
use crate::pystr::simplify_name;
use crate::raw::RawStr;

/// A registration charged and not yet published ([`Registry::prepare`]):
/// its key, its position, its slot (its rank the value's), the share of
/// the tables a key new when it was charged paid for, and the charge for
/// its position's path.
pub(crate) struct Staged {
    /// The simplified name: for a key the registry did not hold when the
    /// registration was charged, its own charged copy, which the key keeps
    /// if it is still new at the lock (the name `simplify_name` built is
    /// dropped once copied, so a stage holds one copy of each new key, not
    /// two); for a known key, the name `simplify_name` built.
    key: RawStr,
    position: Rank,
    slot: Arc<Slot>,
    /// A new key's share of the tables (`key` is then its copy).
    share: Option<Charge>,
    first_charge: Charge,
}

/// A dependency load's registrations, each charged and none published
/// (module docs, "A failed dependency load"): [`Registry::stage`] adds one,
/// [`Registry::commit`] publishes them all in order. Dropped unpublished,
/// it returns every charge and holds no key, so a load that fails before
/// its commit leaves the registry as it was. The list is charged to the
/// dependency's meter.
pub(crate) struct Stage {
    staged: MeteredVec<Staged>,
    /// Whether a staged registration paid for the `order` B-tree's root.
    root: bool,
}

impl Stage {
    /// An empty stage, its list charged to `meter`, the dependency's.
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            staged: MeteredVec::new(meter),
            root: false,
        }
    }

    /// Room for `n` more registrations, charged first.
    pub(crate) fn reserve(&mut self, n: usize) -> Result<(), Error> {
        Ok(self.staged.reserve_exact(n)?)
    }
}

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
    /// its own, charged to `meter` (module docs). The slot holds `cab`, so it
    /// is never evicted: `cab` is a file on a caller's input meter
    /// ([`crate::budget::Reservation::Input`]), not one the Environment
    /// loaded as a dependency, which its load registered already, and
    /// whose reads through such a slot would charge the dependency pool
    /// outside the Environment's retry (plan section 3.13).
    pub fn register(&self, name: &RawStr, cab: Cab, meter: &Arc<Meter>) -> Result<(), Error> {
        let load = self.next_load();
        self.register_ready(name, cab, load, &[], meter)
    }

    /// Registers entry `dir_index` of a bundle the caller loaded `Lazy`, as a
    /// load of its own, to parse on first use; charged to `meter`. As for
    /// [`Self::register`], `bundle` is on a caller's input meter, not a
    /// dependency's.
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
    /// dependency's, one key at a time. The tests' only: a dependency's load
    /// under `Lazy` stages every such registration ([`Self::stage`]), so
    /// that it publishes nothing when it fails (module docs, "A failed
    /// dependency load"); `pass_tests.rs` registers its slots this way.
    #[cfg(test)]
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

    /// Stages a dependency's CAB of load `load` at `path`, charged to
    /// `meter`, the dependency's: [`Self::register_dependency`]'s
    /// registration, or with `lazy` [`Self::register_lazy_dependency`]'s,
    /// every charge made and nothing published until [`Self::commit`].
    // The registration's parts, as `register_lazy_dependency`'s, and its stage.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stage(
        &self,
        stage: &mut Stage,
        name: &RawStr,
        dep: &Arc<Dependency>,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
        lazy: bool,
    ) -> Result<(), Error> {
        let kind = SlotKind::Dependency {
            dep: DepSlot::new(dep),
        };
        let loads = Loads::at(load);
        let staged = self.prepare(name, kind, loads, path, meter, lazy, &mut stage.root)?;
        // Within the room the caller reserved, no growth; past it, a
        // charged one, whose refusal drops the registration with the rest.
        Ok(stage.staged.push(staged)?)
    }

    /// Publishes a load's staged registrations, in the order they were
    /// staged, each as [`Self::insert`] publishes one (and a load's
    /// `settle` after each). Nothing in it can fail the load: every charge
    /// a registration makes was made when it was staged, and the `settle`
    /// after each, which reaches the lazy slots a registration dropped and
    /// charges what it reads, keeps a refusal to itself and leaves the slot
    /// listed for a later call, as every `settle` does. Its one error is
    /// [`Self::apply`]'s for a key that went away after it was found,
    /// which nothing does: no registry call removes a key.
    pub(crate) fn commit(&self, stage: Stage) -> Result<(), Error> {
        let (staged, _list) = stage.staged.into_parts();
        for one in staged {
            self.apply(one, By::Load)?;
        }
        Ok(())
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
    /// returns ([`Self::settle`], module docs), and a change sets the
    /// running container preload walks' flags under the key and, for a
    /// load's, starts a new container index generation, in the step that
    /// publishes it, under the write lock ([`super::watch`], W1;
    /// `RegistryInner::publish`); a load's registration that changes no key
    /// but lists the slot it dropped starts one in the step that lists it.
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
        let staged = self.prepare(name, kind, loads, path, meter, lazy, &mut false)?;
        self.apply(staged, by)
    }

    /// The charged half of [`Self::insert`]: the key, the slot and, for a
    /// key the registry does not hold now, its copy and its share of the
    /// tables (the first key's with the B-tree's root, unless a registration
    /// staged before it paid for that, `root`, which this one sets when it
    /// pays), and the position's path. Every charge a registration makes is
    /// made here, before anything is published: a refusal registers
    /// nothing.
    // The registration's parts, as `insert`'s, and the stage's root.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        name: &RawStr,
        kind: SlotKind,
        loads: Loads,
        path: &[usize],
        meter: &Arc<Meter>,
        lazy: bool,
        root: &mut bool,
    ) -> Result<Staged, Error> {
        let lowered = simplify_name(name, meter)?;
        let slot = Self::new_slot(kind, loads.value, path, lazy, meter)?;
        // The position shares the value's path, which `first_charge` pays
        // for while the key keeps it.
        let position = Rank {
            load: loads.position.min(loads.value),
            path: slot.rank.path.clone(),
        };
        // This read and `apply`'s are two: the one before the charges asks
        // whether the key is new, the one after them reads its value for
        // the stand-in, with no lock held between (the price of charging a
        // staged registration before any is published; `insert` pays it
        // too, one more read lock per registration).
        let (known, first) = {
            let inner = self.read();
            let known = inner.slots.contains_key(lowered.as_bytes());
            (known, inner.order.is_empty() && !*root)
        };
        // A new key's own copy and its share of the tables, made before the
        // lock: the key holds no input's buffer (module docs); the first key
        // also pays for the B-tree's root. The share is charged before the
        // copy. A key found here is found at the lock too, since no call
        // removes one; a key not found here may be by then (another load's,
        // or one this load staged before it), and its copy and share are
        // then returned. A known key may move to this rank: its path is
        // charged now and returned if it does not.
        let (key, share) = if known {
            (lowered, None)
        } else {
            let share = meter.charge(if first { SHARE + ORDER_LEAF } else { SHARE })?;
            *root |= first;
            (RawStr::charged(lowered.as_bytes(), meter)?, Some(share))
        };
        let first_charge = meter.charge(Rank::heap(path.len()))?;
        Ok(Staged {
            key,
            position,
            slot,
            share,
            first_charge,
        })
    }

    /// The published half of [`Self::insert`], which nothing can refuse:
    /// whether the slot that stays stands in for the one dropped, told with
    /// no lock held ([`Stand`]), then the change, in one step under the
    /// write lock, then what it dropped released and, for a load's, the
    /// listed slots reached ([`Self::settle`], whose refusal leaves them
    /// listed for a later call and does not fail the registration). Its one
    /// error is for a key found when the registration was charged and gone
    /// at the lock, which nothing does: no call removes a key.
    fn apply(&self, staged: Staged, by: By) -> Result<(), Error> {
        let Staged {
            key,
            position,
            slot,
            share,
            first_charge,
        } = staged;
        let rank = slot.rank.clone();
        let value = self
            .read()
            .slots
            .get(key.as_bytes())
            .map(|e| e.slot.clone());
        let stand = value.as_ref().and_then(|v| Stand::of(&slot, v));
        drop(value);
        let hash = self.watches.hash(key.as_bytes());
        let (dropped, changed, new_key) = {
            let mut inner = self.write();
            let RegistryInner {
                order,
                slots,
                pending,
                ..
            } = &mut *inner;
            let out = if let Some(e) = slots.get_mut(key.as_bytes()) {
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
            } else if let Some(share) = share {
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
                // The key went away between the charge and the lock: never,
                // since nothing removes one; register it on the next call.
                return Err(Error::Malformed {
                    what: "registry",
                    detail: "a key vanished".into(),
                });
            };
            // The change, the setting of the walks' flags under its key and
            // a load's new generation are one step (`super::watch`, W1).
            // So are a load's listing of the slot it dropped, which changes
            // no key, and a new generation: the reach that settles the slot
            // registers its container's entries, which no pass marked built
            // before this step has walked (plan section 3.16, P1; review
            // round 16: a load ranked between the key's position and its
            // value listed its slot with no new generation, so its
            // container's tables were never walked).
            if out.1 {
                inner.publish(hash, by == By::Load);
            } else if by == By::Load && out.0.as_ref().is_some_and(|d| d.listed.is_some()) {
                inner.start_generation();
            }
            out
        };
        self.count_change(changed, new_key, by);
        drop(stand);
        self.release(dropped);
        if by == By::Load {
            // A refusal leaves it for the next call (`settle`).
            self.settle().ok();
        }
        Ok(())
    }

    /// What a registration counts once its change is published: a load's
    /// new key counts on this thread (`LOAD_KEYS_HERE`); a reach registers
    /// what an eager load registered at load, before any container pass.
    // `changed` is the test's hook's.
    #[cfg_attr(not(test), allow(unused_variables, clippy::unused_self))]
    fn count_change(&self, changed: bool, new_key: bool, by: By) {
        #[cfg(test)]
        if changed {
            self.hook(Hook::Published);
        }
        if new_key && by == By::Load {
            LOAD_KEYS_HERE.with(|c| c.set(c.get().saturating_add(1)));
        }
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

    /// A slot of `kind` at rank `(load, path)`, charged to `meter` (its
    /// `Arc`, its rank's path, and a lazy slot's share of the list of
    /// dropped slots) before either is allocated.
    fn new_slot(
        kind: SlotKind,
        load: u64,
        path: &[usize],
        lazy: bool,
        meter: &Arc<Meter>,
    ) -> Result<Arc<Slot>, Error> {
        let mut charge = Charge::zero(meter);
        charge.grow(SLOT_ARC)?;
        charge.grow(Rank::heap(path.len()))?;
        if lazy {
            charge.grow(PENDING_SHARE)?;
        }
        // The path's `Arc` after its charge (CR1's review, round 1: it was
        // allocated before it).
        let rank = Rank {
            load,
            path: Arc::from(path),
        };
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
