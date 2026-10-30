//! The CAB registry: UnityPy's `env.cabs` dict (`environment.py:252-280`).
//!
//! Keys are [`crate::pystr::simplify_name`] of the registered name. As a
//! Python dict: a repeated key replaces the value and keeps the first
//! position, and iteration is in first-insertion order (`env.container`
//! walks it).
//!
//! **Ranks.** Every registration has a rank (the crate's `Rank`): the
//! sequence number of the load that made it, then the directory indexes from
//! that load's top-level file to the entry (empty for the file itself, `[i]`
//! for entry `i` of a bundle, `[i, j]` for entry `j` of the bundle that is
//! entry `i`). UnityPy's eager load registers a bundle's entries in place, in
//! directory order, so the order in which it registers them is the order of
//! their ranks, and a key's position is its *least* rank and its value the
//! registration of *greatest* rank. The registry keeps exactly that, whatever
//! the order the registrations arrive in: a registration below the key's
//! value's rank leaves the value, one below its position's rank moves the
//! key there, and one of equal rank (the same entry registered again) is no
//! change. So the registry holds what an eager load registers whatever
//! the order its registrations arrive in, from whichever thread.
//!
//! **Nested containers under `LoadMode::Lazy`** (plan section 3.13,
//! difference (b)). A lazy bundle registers every directory entry's name at
//! open, unread, so an entry that is itself a bundle is a key of its own
//! (its slot answers `None`) until it is *reached*: its first bytes are
//! read and, when they are a bundle's, it is parsed and its whole subtree
//! registered in place, at the ranks an eager load gives them (a container
//! inside it has no key, as under `Eager`). One routine reaches
//! (`Registry::reach`, `reach.rs`), and the invariant it keeps is: a
//! nested container is reached before any of these returns: a lookup that
//! resolves its slot (a `get` of its name, a pointer that names it, the
//! container pass); a lookup from a file inside it (a dereference, a
//! resource read: `super::Environment`'s `reach_chain`); a pointer or a
//! resource candidate that reaches it through the asking file's own
//! bundle; a container pass, which reaches every registered entry before
//! it walks; and a registration that would drop its slot (replace it, or
//! lose to a greater rank), which reaches the dropped slot first
//! (`Registry::settle`): nothing could reach it by name afterwards, and
//! an eager load registered its entries at load (review round 4: a later
//! load's entry of the same path dropped it unread, and its entries were
//! never registered). What remains of (b) is a lookup by name alone (a
//! `get_cab`, a `find_file`, a pointer to a file that is not the asking
//! file's sibling) of an entry of a container nothing has reached, which
//! misses or finds an earlier registration of its key, and the container's
//! own name, a key at its rank where an eager load registers none and keeps
//! an earlier CAB of that path under it. Under eager loading a bundle
//! registers its CABs as it parses them, so a later entry's failure leaves
//! the earlier ones registered (difference (a)).
//!
//! **What a slot holds** (the crate's `Slot`). A file the caller loaded is never
//! evicted, so its slot may hold its handle. A dependency's slot never
//! holds a strong handle on it: it holds what *reopens* the dependency (its
//! name, its [`super::Source`], its meter, and the path of directory indexes
//! from its top-level file to the CAB, its rank's) and a `Weak` on the handle
//! the dependency LRU keeps, so evicting the dependency (dropping the LRU's
//! handle) frees its runs, its parsed entries, its nested bundles and, for a
//! source that can be read again (`Path`, `Fetch`), its bytes and their
//! charge; the next `get` opens it again from the source on
//! [`crate::limits::Meter::reopen`] of its meter (the same memory account,
//! the new open's own work allowance, plan section 3.13) and walks the path
//! to the CAB. Parsed entries are cached in the bundle handle's own cells
//! ([`crate::files::BundleFile::entry`]), not in the registry, so they go
//! with the handle. A CAB inside a nested bundle of a dependency is slotted
//! by its path, never by a handle on the inner bundle, which would keep it,
//! and the slice of the outer's decoded run it is made from, alive past the
//! eviction. When the last slot of a dependency goes (every key it
//! registered was registered again by another file), nothing can reach it
//! any more and the LRU drops it. Its slots are counted on it
//! (`DepSlot`), so of two dropped at once exactly one is the last, and a
//! load whose every key was replaced before it reached the LRU lets go of
//! it itself (review round 2: a test of the `Arc` count saw both slots'
//! handles, and neither let go).
//!
//! **Cells.** A slot's parse is published with
//! [`crate::once::get_or_publish_result`]: a budget or meter refusal is
//! returned and not kept, so a shared dependency refused once (while other
//! jobs held the pool) is parsed again on the next `get`. A parse has no side
//! effect but its charges. A nested container's entries are registered from
//! the parsed value by every reach that finds them not yet all registered,
//! and the slot is marked once they all are; a second registration of an
//! entry has the rank of the first and changes nothing, so two threads that
//! reach one container at once both return with every entry registered.
//!
//! **Memory.** Every registration is charged to the meter of the file that
//! registers it: the key ([`crate::raw::RawStr::charged`]), the slot's
//! `Arc` and its rank's path, an element of the `order` B-tree and four
//! buckets of the `slots` hash table (a bucket is a key, a slot handle and a
//! control byte). Every node of std's `BTreeMap` but the root holds at least
//! 5 of its 11 elements, and every internal node but the root at least 6
//! children, so a fifth of a leaf and a thirtieth of an internal node per
//! element cover the tree; a doubling hash table holds up to 16/7 buckets per
//! key at rest and 24/7 while it grows (the crate's `HASH_TABLE_SHARES`), so
//! the tables' spare capacity and every growth are paid for. The first key
//! also pays for the B-tree's root leaf. A lazy entry's slot also pays for
//! its place in the list of dropped slots waiting to be reached
//! (`Registry::settle`). `tests/env_memory.rs` measures the
//! heap against the charge. A refused charge registers nothing and is the
//! registering load's error.
//!
//! Owner: package D.

mod dependency;
mod lru;
mod rank;
mod reach;
mod slot;

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;
use std::ops::Bound;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

pub(crate) use dependency::{DepSlot, Dependency, WeakFile, open_source};
pub(crate) use lru::DepCache;
#[cfg(test)]
pub(crate) use lru::{Hook, HookFn};
pub(crate) use rank::Rank;
pub(crate) use slot::{Slot, SlotKind};

use super::watch::Watches;

use crate::budget::Reservation;
use crate::error::Error;
use crate::files::{BundleFile, File, RawFile, SerializedFile};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};
use crate::pystr::simplify_name;
use crate::raw::RawStr;

/// A registered CAB: what `get_cab` returns.
#[derive(Clone, Debug)]
pub enum Cab {
    /// A `SerializedFile`.
    Serialized(Arc<SerializedFile>),
    /// A raw reader (`.resS`, `.resource`, name-rule files).
    Raw(RawCab),
}

/// A raw CAB's bytes.
#[derive(Clone, Debug)]
pub enum RawCab {
    /// A bundle entry, read through the bundle (no copy of the whole entry).
    InBundle {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
    },
    /// A file loaded on its own.
    File(Arc<RawFile>),
}

/// A CAB with the handle that keeps its container alive: for a dependency's
/// CAB, the dependency's top-level file as the lookup reached it (an
/// eviction drops only the LRU's handle, so while this one is held the CAB's
/// bundle, and with it the pointers that need its siblings, stay
/// reachable); `None` for a CAB of a file the caller loaded, which is never
/// evicted. What [`super::Environment::get_cab_held`] returns.
#[derive(Clone, Debug)]
pub struct HeldCab {
    /// The CAB.
    pub cab: Cab,
    /// The dependency's top-level file, when the CAB is a dependency's.
    pub hold: Option<File>,
}

const _: () = crate::send_sync::<HeldCab>();

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One key: the slot of greatest rank, the key's position (the least rank
/// registered under it), and the charges for its share of the two tables
/// and for its position's path.
#[derive(Debug)]
struct Entry {
    slot: Arc<Slot>,
    first: Rank,
    _share: Charge,
    /// The charge for `first`'s path.
    first_charge: Charge,
}

/// The registry.
#[derive(Debug)]
pub struct Registry {
    inner: RwLock<RegistryInner>,
    /// The next load's sequence number.
    loads: AtomicU64,
    /// Bumped by every load's registration that changes the dict: UnityPy's
    /// `register_cab` clears `_container_index_built` (`environment.py:264`).
    generation: AtomicU64,
    /// Lazy slots a registration dropped unreached, to reach
    /// ([`Self::settle`]), and how many there are.
    pending: Mutex<Vec<Arc<Slot>>>,
    pending_len: AtomicUsize,
    /// The generation the last finished container pass started at (0: none).
    built: AtomicU64,
    /// Entries parsed through slots, for `EnvStats`.
    parsed: AtomicU64,
    /// The running container preload walks' watches on keys.
    watches: Watches,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            inner: RwLock::default(),
            loads: AtomicU64::new(0),
            generation: AtomicU64::new(1),
            pending: Mutex::default(),
            pending_len: AtomicUsize::new(0),
            built: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
            watches: Watches::default(),
        }
    }
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// Each key at its position.
    order: BTreeMap<Rank, RawStr>,
    slots: HashMap<RawStr, Entry>,
}

/// A B-tree node of `order`: its header, 11 keys and 11 values; an internal
/// node adds 12 edges (std's `BTreeMap`, `B = 6`).
const ORDER_ELEMENT: usize = size_of::<Rank>() + size_of::<RawStr>();
const ORDER_LEAF: u64 = heap_cost((16 + 11 * ORDER_ELEMENT) as u64);
const ORDER_INTERNAL: u64 = heap_cost((16 + 11 * ORDER_ELEMENT + 12 * size_of::<usize>()) as u64);

/// What one more key costs the two tables: its share of the `order` B-tree
/// (a fifth of a leaf, a thirtieth of an internal node) and
/// [`HASH_TABLE_SHARES`] buckets of `slots` (module docs).
const SHARE: u64 = ORDER_LEAF / 5
    + 1
    + ORDER_INTERNAL / 30
    + 1
    + HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<Entry>() + 1) as u64;

/// A slot's own allocation: the `Slot` in its `Arc`.
const SLOT_ARC: u64 = heap_cost((size_of::<Slot>() + 2 * size_of::<usize>()) as u64);

/// A lazy slot's share of the list of dropped slots waiting to be reached:
/// four elements, the most the list keeps per element it holds
/// (`Registry::settle`).
const PENDING_SHARE: u64 = 4 * size_of::<Arc<Slot>>() as u64;

thread_local! {
    /// New keys loads made on this thread: the container pass counts the
    /// ones its own dereferences make (UnityPy's `RuntimeError: dictionary
    /// changed size during iteration`), never another thread's.
    static LOAD_KEYS_HERE: Cell<u64> = const { Cell::new(0) };
}

/// New keys loads have made on this thread so far.
pub(crate) fn load_keys_here() -> u64 {
    LOAD_KEYS_HERE.with(Cell::get)
}

/// Who makes a registration: a load (a new key counts for the container
/// pass's `RuntimeError`), or the reach of a nested container.
#[derive(Clone, Copy, PartialEq, Eq)]
enum By {
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
            load,
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
        self.insert(name, kind, load, path, meter, By::Load, true)
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
        self.insert(name, kind, load, path, meter, By::Load, false)
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
        self.insert(name, kind, load, path, meter, By::Load, true)
    }

    /// Registers `kind` at rank `(load, path)` under `simplify_name(name)`
    /// with dict semantics by rank (module docs): a new key takes its place
    /// in the order and pays its share of the tables and a copy of itself; a
    /// known key takes the slot only if its rank is greater than its slot's,
    /// moves to the rank if it is less than its position's, and pays only for
    /// the slot (and its new position's path when it moves). Charged before
    /// anything is allocated; a refusal registers nothing. A lazy slot the
    /// registration drops unreached (the value it replaces, or its own slot
    /// when that ranks below the value) is reached before it returns
    /// ([`Self::settle`], module docs), and a change is noted for the
    /// running container preload walks ([`super::watch`]).
    // The registration's parts, each one of the module docs' terms.
    #[allow(clippy::too_many_arguments)]
    fn insert(
        &self,
        name: &RawStr,
        kind: SlotKind,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
        by: By,
        lazy: bool,
    ) -> Result<(), Error> {
        // UnityPy environment.py:252-264
        let lowered = simplify_name(name, meter)?;
        let rank_heap = Rank::heap(path.len());
        let mut charge = Charge::zero(meter);
        charge.grow(SLOT_ARC)?;
        charge.grow(rank_heap)?;
        if lazy {
            charge.grow(PENDING_SHARE)?;
        }
        let rank = Rank {
            load,
            path: Arc::from(path),
        };
        let slot = Arc::new(Slot {
            kind,
            rank: rank.clone(),
            lazy,
            reached: AtomicBool::new(false),
            counted: AtomicBool::new(false),
            walked: AtomicBool::new(false),
            _charge: charge,
        });
        let (known, first) = {
            let inner = self.read();
            (
                inner.slots.contains_key(lowered.as_bytes()),
                inner.order.is_empty(),
            )
        };
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
        let first_charge = meter.charge(rank_heap)?;
        let (dropped, changed, new_key) = {
            let mut inner = self.write();
            let RegistryInner { order, slots } = &mut *inner;
            if let Some(e) = slots.get_mut(lowered.as_bytes()) {
                let moves = rank < e.first;
                let charge = if moves {
                    if let Some(key) = order.remove(&e.first) {
                        order.insert(rank.clone(), key);
                    }
                    e.first = rank.clone();
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
                let unreached = distinct && slot.unreached() && !reach::same_entry(&slot, &e.slot);
                (Some((slot, charge, unreached)), moves || replaces, false)
            } else if let Some((key, share)) = fresh {
                order.insert(rank.clone(), key.clone());
                slots.reserve(1);
                slots.insert(
                    key,
                    Entry {
                        slot,
                        first: rank,
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
        // What the registration replaced or did not use (and its charges)
        // is dropped after the lock; a lazy slot not yet reached is reached
        // first (module docs).
        if let Some((slot, charge, true)) = dropped {
            drop(charge);
            self.defer(slot);
            // A refusal leaves it for the next call (`settle`).
            self.settle().ok();
            return Ok(());
        }
        drop(dropped);
        Ok(())
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, RegistryInner> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, RegistryInner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// The slot under `simplify_name(name)`, the key built charged to `meter`
    /// (no copy when `name` is already simplified) and dropped on return,
    /// after the dropped lazy slots are reached ([`Self::settle`]).
    pub(crate) fn slot(
        &self,
        name: &RawStr,
        meter: &Arc<Meter>,
    ) -> Result<Option<Arc<Slot>>, Error> {
        // A refusal leaves it for the next call (`settle`).
        self.settle().ok();
        let key = simplify_name(name, meter)?;
        Ok(self
            .read()
            .slots
            .get(key.as_bytes())
            .map(|e| e.slot.clone()))
    }

    /// The first key after `after` in the order (the first of all for
    /// `None`), its position and its slot: a cursor that keys registered
    /// meanwhile do not disturb.
    pub(crate) fn slot_after(&self, after: Option<&Rank>) -> Option<(Rank, Arc<Slot>)> {
        let inner = self.read();
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let (rank, key) = inner.order.range((lower, Bound::Unbounded)).next()?;
        let slot = inner.slots.get(key.as_bytes())?.slot.clone();
        Some((rank.clone(), slot))
    }

    /// The first key after `after` in the order, and its position.
    fn key_after(&self, after: Option<&Rank>) -> Option<(Rank, RawStr)> {
        let inner = self.read();
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let (rank, key) = inner.order.range((lower, Bound::Unbounded)).next()?;
        Some((rank.clone(), key.clone()))
    }

    /// The generation now (bumped by every change to the dict).
    /// The running container preload walks' watches on keys.
    pub(crate) fn watches(&self) -> &Watches {
        &self.watches
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Whether a container pass that started at `generation` has finished.
    pub(crate) fn built(&self, generation: u64) -> bool {
        self.built.load(Ordering::Acquire) >= generation
    }

    /// Publishes a finished container pass that started at `generation`:
    /// true when it is the first to, false when one that started at it (or
    /// later) already finished.
    pub(crate) fn publish_built(&self, generation: u64) -> bool {
        self.built.fetch_max(generation, Ordering::AcqRel) < generation
    }

    fn count(&self, slot: &Slot) {
        if !slot.counted.swap(true, Ordering::Relaxed) {
            self.parsed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Entries parsed through slots so far.
    pub(crate) fn parsed(&self) -> u64 {
        self.parsed.load(Ordering::Relaxed)
    }

    /// `get_cab(name)`, parsing a lazy slot now, or opening an evicted
    /// dependency again from its source. The key is `simplify_name(name)`,
    /// built charged to `meter` (`crate::pystr::simplify_name`) and dropped
    /// when the lookup returns; a name already in its simplified form is
    /// looked up as it is, with no copy.
    pub fn get(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        Ok(self.get_held(name, meter)?.map(|h| h.cab))
    }

    /// [`Self::get`], with the handle that keeps a dependency's CAB's
    /// container alive ([`HeldCab`]).
    pub(crate) fn get_held(
        &self,
        name: &RawStr,
        meter: &Arc<Meter>,
    ) -> Result<Option<HeldCab>, Error> {
        match self.slot(name, meter)? {
            Some(slot) => self.resolve_held(&slot),
            None => Ok(None),
        }
    }

    /// What a slot answers: its CAB, parsed through its bundle's own cell
    /// (`Lazy`) or reached from its dependency's open handle (reopened from
    /// its source after an eviction); `None` for a nested container, which
    /// is reached then (module docs). A dependency read the pool refuses
    /// evicts the others and retries once.
    pub(crate) fn resolve(&self, slot: &Arc<Slot>) -> Result<Option<Cab>, Error> {
        Ok(self.resolve_held(slot)?.map(|h| h.cab))
    }

    /// [`Self::resolve`], with the dependency's top-level file ([`HeldCab`]).
    pub(crate) fn resolve_held(&self, slot: &Arc<Slot>) -> Result<Option<HeldCab>, Error> {
        let held = |cab: Option<Cab>| cab.map(|cab| HeldCab { cab, hold: None });
        match &slot.kind {
            SlotKind::Ready(cab) => Ok(held(Some(cab.clone()))),
            SlotKind::Entry { bundle, dir_index } => {
                let file = bundle.entry(*dir_index)?;
                self.count(slot);
                self.answer(slot, file, Some((bundle, *dir_index)))
                    .map(held)
            }
            SlotKind::Dependency { dep } => {
                let first = self.resolve_dependency(slot, dep);
                match first {
                    Err(Error::Budget(b)) if b.what == Reservation::Dependency => {
                        if let Some(cache) = dep.cache.upgrade() {
                            cache.evict_others(Some(dep));
                        }
                        self.resolve_dependency(slot, dep)
                    }
                    other => other,
                }
            }
        }
    }

    fn resolve_dependency(
        &self,
        slot: &Arc<Slot>,
        dep: &Arc<Dependency>,
    ) -> Result<Option<HeldCab>, Error> {
        let top = dep.handle()?;
        let mut file = top.clone();
        let mut at: Option<(Arc<BundleFile>, usize)> = None;
        let path = slot.rank.path();
        for &i in path {
            let File::Bundle(b) = file else {
                return Ok(None);
            };
            file = b.entry(i)?;
            at = Some((b, i));
        }
        if !path.is_empty() {
            self.count(slot);
        }
        // The walk's parse charged after the touch: count it now.
        if let Some(cache) = dep.cache.upgrade() {
            cache.refresh(dep);
        }
        let at = at.as_ref().map(|(b, i)| (b, *i));
        let cab = self.answer(slot, file, at)?;
        Ok(cab.map(|cab| HeldCab {
            cab,
            hold: Some(top),
        }))
    }

    /// The CAB a slot's file is: a `SerializedFile`, or a raw reader (read
    /// through its bundle when it is an entry); a nested container is none,
    /// and is reached then ([`Self::reach`]).
    fn answer(
        &self,
        slot: &Arc<Slot>,
        file: File,
        at: Option<(&Arc<BundleFile>, usize)>,
    ) -> Result<Option<Cab>, Error> {
        let cab = match file {
            File::Serialized(sf) => Some(Cab::Serialized(sf)),
            File::Raw(raw) => Some(Cab::Raw(match at {
                Some((bundle, dir_index)) => RawCab::InBundle {
                    bundle: bundle.clone(),
                    dir_index,
                },
                None => RawCab::File(raw),
            })),
            File::Bundle(nested) => {
                self.reach(slot, Some(&nested))?;
                return Ok(None);
            }
            File::Web(_) => None,
        };
        slot.mark_reached();
        Ok(cab)
    }

    /// Keys in dict order, as an iterator that takes the read lock briefly
    /// per key (no snapshot of the registry).
    #[must_use]
    pub fn keys(&self) -> Keys<'_> {
        Keys {
            registry: self,
            after: None,
        }
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.read().order.len()
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The iterator [`Registry::keys`] returns.
#[derive(Debug)]
pub struct Keys<'a> {
    registry: &'a Registry,
    after: Option<Rank>,
}

impl Iterator for Keys<'_> {
    type Item = RawStr;

    fn next(&mut self) -> Option<RawStr> {
        let (rank, key) = self.registry.key_after(self.after.as_ref())?;
        self.after = Some(rank);
        Some(key)
    }
}

#[cfg(test)]
mod tests;
