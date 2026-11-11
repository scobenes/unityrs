// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
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
//! lose to a greater rank), which lists the dropped slot, and whose load
//! reaches it before it returns (`Registry::settle`): nothing could reach
//! it by name afterwards, and an eager load registered its entries at load
//! (review round 4: a later load's entry of the same path dropped it
//! unread, and its entries were never registered). Every reader settles
//! first (a load's registration, a lookup by name, the key listing, the
//! container pass), and every `settle` reaches every listed slot, so none
//! returns while a slot listed before it is unreached, but for a refusal
//! (plan section 3.16, R3; `reach.rs`). A dropped slot of a dependency the
//! LRU evicted is not opened again by a `settle`: it waits on a list of its
//! own for the container pass, which reopens the dependency and reaches it
//! (plan 9, Q13, revised after review round 6). A dropped slot is not
//! reached when the key's value stands in for it: the same entry of the
//! same input, as every handle on the dropped entry can tell
//! (`reach::stands_in`), which a lookup from a file inside the dropped entry
//! then reaches, and whose reach registers the entries at the dropped
//! slot's positions (plan 9, Q14, revised after review round 6;
//! `register.rs`, `Stand`), until that reach closes its position, after
//! which a dropped slot is listed (`reach.rs`, "Stand-ins").
//! What remains of (b) is a lookup by name alone (a
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
//! **Memory.** Every registration is charged to the meter of the input its
//! slot belongs to (plan section 3.16, R2): a dependency's slots, those a
//! reach through another open's handle registers included, to the
//! dependency's own meter, any other to the registering file's: the key ([`crate::raw::RawStr::charged`]), the slot's
//! `Arc` and its rank's path, an element of the `order` B-tree and four
//! buckets of the `slots` hash table (a bucket is a key, a slot handle and a
//! control byte). Every node of std's `BTreeMap` but the root holds at least
//! 5 of its 11 elements, and every internal node but the root at least 6
//! children, so a fifth of a leaf and a thirtieth of an internal node per
//! element cover the tree; a doubling hash table holds up to 16/7 buckets per
//! key at rest and 24/7 while it grows (the crate's `HASH_TABLE_SHARES`), so
//! the tables' spare capacity and every growth are paid for. The first key
//! also pays for the B-tree's root leaf. A lazy entry's slot also pays for
//! its share of a node of the lists of dropped slots waiting to be reached
//! (`pending.rs`); each list's root, one node at most and none once it
//! empties, is a constant of the registry, as the hash table's control
//! tail is (plan section 3.13). A dependency's `Lazy` load holds what it
//! staged until it publishes it (below, "A failed dependency load"): the
//! list, charged to the dependency's meter, and each registration's
//! charges, a key new when staged paying its copy and share even when an
//! earlier row of the same load takes the key first (returned at the
//! commit). `tests/env_memory.rs` measures the heap against the charge, a
//! lazy dependency's load included. A refused charge registers nothing and
//! is the registering load's error.
//!
//! **A failed dependency load** (plan section 3.13; `docs/plans/batch.md`
//! section 6, CR1). Under `LoadMode::Lazy` a dependency's load
//! (`Environment::load_dependency`, or a resolver's load made as a
//! dependency) publishes nothing when it fails. It stages its
//! registrations (`register.rs`, `Stage`): the list that holds them, room
//! for a bundle's whole directory charged before any entry, then each
//! registration charged as any is (`prepare`: its key, its slot, a new
//! key's share and copy, its position's path); then the LRU's entry, and a
//! `find_file` load's `files` entry under `keep_resolver_loads`, are
//! charged; only then are the registrations published, one key at a time
//! and in the order they were staged, each as any load's registration is
//! (`apply`, then the load's `settle`), and nothing in that can fail the
//! load: the `settle` reaches the lazy slots a registration dropped and
//! charges what it reads, and keeps a refusal there to itself, the slot
//! left listed for a later call, as every `settle` does. A refusal before
//! the commit drops the stage, which returns every charge and was never in
//! the registry: no lookup, on this thread or another, can have found any
//! of it, no slot was dropped or listed, and no generation or walk's flag
//! moved, so the registry is as it was before the load. It does not cover
//! a caller's load (`Environment::load`, and a resolver's load made as a
//! caller's under `keep_resolver_loads`), which registers one key at a
//! time under either mode and keeps, when it fails, those registered
//! before. Under `LoadMode::Eager` a dependency's CABs are registered as
//! they are parsed, as UnityPy's `load_file(..., is_dependency=True)`
//! registers them (`files/File.py:76-90` registers each entry whatever the
//! flag), so a load that fails keeps those registered before its failure
//! (the partial registration above, difference (a)).
//!
//! Owner: package D.

mod cab;
mod dependency;
mod lru;
mod pending;
mod rank;
mod reach;
mod register;
mod slot;

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;
use std::ops::Bound;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

pub use cab::{Cab, HeldCab, RawCab};
pub(crate) use dependency::{
    DepSlot, Dependency, Open, PoolRefusal, from_callers, open_source, with_room,
};
pub(crate) use lru::DepCache;
#[cfg(test)]
pub(crate) use lru::{Hook, HookFn};
pub(crate) use rank::Rank;
pub(crate) use reach::{Through, entry_of};
pub(crate) use register::Stage;
use register::{By, Loads};
pub(crate) use slot::{Slot, SlotKind};

use super::held::{self, Held, lock};
use super::watch::{Table, Under, Watches};

use crate::error::Error;
use crate::files::{BundleFile, File};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};
use crate::pystr::simplify_name;
use crate::raw::RawStr;

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
    /// How many lazy slots `RegistryInner::pending` lists for every
    /// `settle` ([`pending::List::Listed`]).
    pending_len: AtomicUsize,
    /// How many it keeps for the container pass
    /// ([`pending::List::Evicted`]).
    evicted_len: AtomicUsize,
    /// The generation the last finished container pass started at (0: none).
    built: AtomicU64,
    /// Entries parsed through slots, for `EnvStats`.
    parsed: AtomicU64,
    /// The running container preload walks' watches on keys.
    watches: Watches,
    /// Where a test stops a thread ([`Hook`]).
    #[cfg(test)]
    pub(crate) hook: Mutex<Option<HookFn>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            inner: RwLock::default(),
            loads: AtomicU64::new(0),
            pending_len: AtomicUsize::new(0),
            evicted_len: AtomicUsize::new(0),
            built: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
            watches: Watches::default(),
            #[cfg(test)]
            hook: Mutex::new(None),
        }
    }
}

impl Under for Registry {
    /// Under the registry's write lock, which publishes every key's change
    /// (`register.rs`).
    fn with_table(&self, f: &mut dyn FnMut(&mut Table)) {
        f(&mut self.write().watched);
    }
}

#[derive(Debug)]
struct RegistryInner {
    /// Each key at its position.
    order: BTreeMap<Rank, RawStr>,
    slots: HashMap<RawStr, Entry>,
    /// The lazy slots dropped unreached ([`Registry::settle`]).
    pending: pending::Pending,
    /// The running container preload walks' flags under registry keys,
    /// set in the step that publishes a key's change (`super::watch`).
    watched: Table,
    /// The container index generation: a new one starts in the step that
    /// publishes a load's change to the dict ([`Self::publish`]), as
    /// UnityPy's `register_cab` clears `_container_index_built` in the
    /// statement after the one that sets the key (`environment.py:263-264`),
    /// and in the step in which a load's registration that changes no key
    /// lists the lazy slot it dropped (`register.rs`), whose reach registers
    /// what an eager load registered at that load.
    generation: u64,
}

impl Default for RegistryInner {
    fn default() -> Self {
        Self {
            order: BTreeMap::new(),
            slots: HashMap::new(),
            pending: pending::Pending::default(),
            watched: Table::default(),
            generation: 1,
        }
    }
}

impl RegistryInner {
    /// The step that publishes a change to the key hashed `hash`, under the
    /// write lock that made it: the walks' flags under it set
    /// (`super::watch`, W1) and, for a load's (`load`), a new container
    /// index generation started (plan section 3.16, P1), so a thread that
    /// finds the change also finds every answer read under the key's old
    /// value invalidated (review round 15: the generation was bumped after
    /// the lock, so a thread that found a new key could still read the old
    /// generation's built index).
    fn publish(&mut self, hash: u64, load: bool) {
        self.watched.notify(hash);
        if load {
            self.start_generation();
        }
    }

    /// A new container index generation, under the write lock: for a
    /// load's change ([`Self::publish`]), or for a load's listing of a slot
    /// it dropped (`register.rs`), whose reach registers what no pass
    /// marked built before it has walked.
    fn start_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }
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

/// A lazy slot's share of the lists of dropped slots waiting to be reached
/// (`pending.rs`).
const PENDING_SHARE: u64 = pending::SHARE;

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

impl Registry {
    /// Runs the test's hook at `at`, with no lock held.
    #[cfg(test)]
    pub(crate) fn hook(&self, at: Hook) {
        let hook = lock(&self.hook).clone();
        if let Some(HookFn(f)) = hook {
            f(at);
        }
    }

    fn read(&self) -> Held<std::sync::RwLockReadGuard<'_, RegistryInner>> {
        held::read(&self.inner)
    }

    fn write(&self) -> Held<std::sync::RwLockWriteGuard<'_, RegistryInner>> {
        held::write(&self.inner)
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

    /// The hashing of the running container preload walks' watched keys.
    pub(crate) fn watches(&self) -> &Watches {
        &self.watches
    }

    /// The table of the walks' flags under registry keys, for a test.
    #[cfg(test)]
    pub(crate) fn watch_room(&self) -> (usize, usize, Vec<(usize, usize)>) {
        self.read().watched.room()
    }

    /// The generation now (a new one at every load's change to the dict,
    /// and at every load's listing of a slot it dropped, in the step that
    /// publishes it).
    pub(crate) fn generation(&self) -> u64 {
        self.read().generation
    }

    /// Whether a container pass that started at `generation` has finished.
    pub(crate) fn built(&self, generation: u64) -> bool {
        self.built.load(Ordering::Acquire) >= generation
    }

    /// Publishes a finished container pass that started at `generation`;
    /// [`Self::built`] answers by the greatest one published.
    pub(crate) fn publish_built(&self, generation: u64) {
        self.built.fetch_max(generation, Ordering::AcqRel);
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
    /// looked up as it is, with no copy. The registry holds no LRU, so a
    /// pool refusal of the key's charge, when `meter` is a dependency's, is
    /// returned here as it is: `Environment::get_cab` is the lookup that
    /// evicts and retries (the registry's `with_room`).
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
    /// evicts the others and retries once ([`with_room`]).
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
            SlotKind::Dependency { dep } => dep.with_room(|| self.resolve_dependency(slot, dep)),
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
    /// per key (no snapshot of the registry), after the dropped lazy slots
    /// are reached (`settle`, module docs).
    #[must_use]
    pub fn keys(&self) -> Keys<'_> {
        // A refusal leaves it for the next call (`settle`).
        self.settle().ok();
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
mod pass_tests;
#[cfg(test)]
mod reach_tests;
#[cfg(test)]
mod room_tests;
#[cfg(test)]
mod tests;
