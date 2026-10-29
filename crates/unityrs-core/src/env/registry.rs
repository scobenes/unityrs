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
//! change. So a lazy bundle (`LoadMode::Lazy`), which registers every
//! directory entry's name at open and a nested container's entries when that
//! entry is first asked for, gives each nested entry the position and the
//! value an eager load gives it, whichever nested container was asked for
//! first and from whichever thread (plan section 3.13, difference (b)). What
//! remains of (b) is the nested container's own name: a key under `Lazy`
//! (its slot answers `None`), where an eager load registers no such key and
//! keeps an earlier CAB of that path under it. Under eager loading a bundle
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
//! any more and the LRU drops it.
//!
//! **Cells.** A slot's parse is published with
//! [`crate::once::get_or_publish_result`]: a budget or meter refusal is
//! returned and not kept, so a shared dependency refused once (while other
//! jobs held the pool) is parsed again on the next `get`. A parse has no side
//! effect but its charges. A nested container's entries are registered from
//! the parsed value by every `get` that finds them not yet all registered,
//! and the slot is marked once they all are; a second registration of an
//! entry has the rank of the first and changes nothing, so two threads that
//! expand one container at once both return with every entry registered.
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
//! also pays for the B-tree's root leaf. `tests/env_memory.rs` measures the
//! heap against the charge. A refused charge registers nothing and is the
//! registering load's error.
//!
//! Owner: package D.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;
use std::ops::Bound;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use super::Source;
use crate::budget::Reservation;
use crate::detect::check_file_type;
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, RawFile, SerializedFile, parse_file};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};
use crate::options::FileContext;
use crate::pystr::{is_split_path, simplify_name};
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

/// Where a registration falls in the order an eager load registers it
/// (module docs): the load's sequence number, then the directory indexes
/// from the load's top-level file to the entry.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Rank {
    load: u64,
    path: Arc<[usize]>,
}

impl Rank {
    /// The directory indexes from the load's top-level file.
    pub(crate) fn path(&self) -> &[usize] {
        &self.path
    }

    /// The heap a rank's path takes: its `Arc` in a glibc chunk.
    fn heap(path_len: usize) -> u64 {
        let bytes = 2 * size_of::<usize>() + size_of::<usize>().saturating_mul(path_len);
        heap_cost(u64::try_from(bytes).unwrap_or(u64::MAX))
    }
}

/// A dependency the Environment loaded (through the resolver, or
/// `Environment::load_dependency`): what reopens it, and the handle the LRU
/// keeps while it is cached.
#[derive(Debug)]
pub(crate) struct Dependency {
    /// The name it was loaded under.
    pub(crate) name: RawStr,
    /// Where its bytes come from again after an eviction.
    pub(crate) source: Source,
    /// The meter of its current open. An eviction keeps it; the next open
    /// is on its [`Meter::reopen`], which charges the same memory account
    /// and has its own work allowance, so an old open's handle that someone
    /// still holds keeps counting against the old one (plan section 3.13).
    pub(crate) meter: Mutex<Arc<Meter>>,
    /// The LRU's handle, weakly: an eviction drops the LRU's strong one.
    pub(crate) open: Mutex<Option<WeakFile>>,
    /// The context it is parsed with.
    pub(crate) ctx: Arc<FileContext>,
    /// The LRU that keeps its open handle, weakly: the LRU holds the
    /// dependency, so a strong handle back would keep both alive after the
    /// Environment drops.
    pub(crate) cache: Weak<DepCache>,
}

/// A weak handle on a dependency's top-level file.
#[derive(Debug)]
pub(crate) enum WeakFile {
    /// A `SerializedFile`.
    Serialized(Weak<SerializedFile>),
    /// A bundle.
    Bundle(Weak<BundleFile>),
    /// A raw file.
    Raw(Weak<RawFile>),
}

impl WeakFile {
    pub(crate) fn of(file: &File) -> Option<Self> {
        Some(match file {
            File::Serialized(f) => Self::Serialized(Arc::downgrade(f)),
            File::Bundle(f) => Self::Bundle(Arc::downgrade(f)),
            File::Raw(f) => Self::Raw(Arc::downgrade(f)),
            File::Web(_) => return None,
        })
    }

    fn upgrade(&self) -> Option<File> {
        match self {
            Self::Serialized(w) => w.upgrade().map(File::Serialized),
            Self::Bundle(w) => w.upgrade().map(File::Bundle),
            Self::Raw(w) => w.upgrade().map(File::Raw),
        }
    }
}

/// Reads a source's bytes on `meter` (a [`Source::Path`] through
/// [`super::read_path`], charged first; a [`Source::Fetch`] from its
/// fetcher; [`Source::Bytes`] as they are), detects them and parses them as
/// a top-level file named `name`. A path UnityPy would read as the parts of
/// a split file (`environment.reSplit`, [`is_split_path`]) is refused with
/// [`Unsupported::Split`] before anything is read: UnityPy merges the parts
/// (`environment.py:111-116`), which P1 does not (plan section 1).
pub(crate) fn open_source(
    name: &RawStr,
    source: &Source,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
) -> Result<File, Error> {
    let data = match source {
        Source::Bytes { data, .. } => data.clone(),
        Source::Path(p) => {
            // UnityPy environment.py:111-127: a `.splitN` name, or a missing
            // path whose `.split0` exists.
            let parts = || {
                let mut first = p.clone().into_os_string();
                first.push(".split0");
                !p.exists() && std::path::Path::new(&first).exists()
            };
            if is_split_path(p.as_os_str().as_encoded_bytes()) || parts() {
                return Err(Error::Unsupported(Unsupported::Split));
            }
            super::read_path(p, &ctx.limits, meter)?
        }
        Source::Fetch { fetch, .. } => fetch.fetch()?,
    };
    // UnityPy environment.py:139-154
    let ty = check_file_type(&data, &ctx.limits)?;
    parse_file(name.clone(), data, ty, None, meter, ctx)
}

impl Dependency {
    /// The handle the LRU keeps, opening the dependency again from its
    /// source when it was evicted (on [`Meter::reopen`] of its meter). The
    /// source is read and parsed with no lock held; the first open to
    /// publish wins.
    pub(crate) fn handle(self: &Arc<Self>) -> Result<File, Error> {
        let alive = lock(&self.open).as_ref().and_then(WeakFile::upgrade);
        if let Some(f) = alive {
            if let Some(cache) = self.cache.upgrade() {
                cache.touch(self, &f)?;
            }
            return Ok(f);
        }
        let meter = lock(&self.meter).reopen();
        let file = open_source(&self.name, &self.source, &meter, &self.ctx)?;
        let file = {
            let mut open = lock(&self.open);
            if let Some(theirs) = open.as_ref().and_then(WeakFile::upgrade) {
                theirs
            } else {
                *open = WeakFile::of(&file);
                *lock(&self.meter) = meter;
                file
            }
        };
        if let Some(cache) = self.cache.upgrade() {
            cache.touch(self, &file)?;
        }
        Ok(file)
    }

    /// The open handle if it is alive, without opening anything.
    pub(crate) fn alive(&self) -> Option<File> {
        lock(&self.open).as_ref().and_then(WeakFile::upgrade)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The dependency LRU: the strong handle on each cached dependency, most
/// recently used last, bounded by the sum of their meters
/// (`EnvOptions::dep_cache_bytes`). Locked only to reorder, to insert or to
/// take handles out; a handle it drops is dropped after the lock.
///
/// Every operation is O(1) but an eviction's, which is O(1) per dependency
/// it evicts: the entries are a doubly linked list threaded through a hash
/// map (keyed by the dependency's address, which its `Arc` in the entry
/// keeps), and the bound is checked against the sum of each entry's meter
/// as it stood when the entry was last used (touched, or refreshed after a
/// read through it), re-read for the least recently used entry before it is
/// evicted. A meter that grew or shrank since its entry's last use is not
/// re-read until then, so the sum can lag a use behind; the pool's own
/// refusal (an eviction and one retry, plan section 3.13) is what bounds the
/// memory, and the LRU only orders what goes first.
#[derive(Debug)]
pub(crate) struct DepCache {
    bound: u64,
    /// False when nothing is ever evicted, a pool refusal included
    /// ([`super::EnvOptions::dep_cache_bytes`]).
    evicts: bool,
    lru: Mutex<Lru>,
    pub(crate) loaded: AtomicU64,
    pub(crate) evicted: AtomicU64,
    /// Entries visited, for the tests that count the LRU's work.
    #[cfg(test)]
    steps: AtomicU64,
}

/// One cached dependency.
#[derive(Debug)]
struct LruNode {
    /// Keeps the dependency, and so the address that keys the node.
    _dep: Arc<Dependency>,
    file: File,
    /// Its meter as it stood when it was last used.
    seen: u64,
    prev: Option<usize>,
    next: Option<usize>,
    /// Its share of the map, charged to its meter.
    _charge: Charge,
}

#[derive(Debug, Default)]
struct Lru {
    nodes: HashMap<usize, LruNode>,
    /// The least recently used.
    head: Option<usize>,
    /// The most recently used.
    tail: Option<usize>,
    /// The sum of every node's `seen`.
    total: u64,
}

/// What one cached dependency costs the map: [`HASH_TABLE_SHARES`] buckets.
const LRU_SHARE: u64 = HASH_TABLE_SHARES * (size_of::<usize>() + size_of::<LruNode>() + 1) as u64;

/// The LRU's key for `dep`: its address, which the entry's `Arc` keeps.
fn lru_key(dep: &Arc<Dependency>) -> usize {
    Arc::as_ptr(dep).addr()
}

impl Lru {
    /// Takes `k` out of the list (it stays in the map).
    fn unlink(&mut self, k: usize) {
        let Some((prev, next)) = self.nodes.get(&k).map(|n| (n.prev, n.next)) else {
            return;
        };
        match prev.and_then(|p| self.nodes.get_mut(&p)) {
            Some(p) => p.next = next,
            None => self.head = next,
        }
        match next.and_then(|n| self.nodes.get_mut(&n)) {
            Some(n) => n.prev = prev,
            None => self.tail = prev,
        }
        if let Some(n) = self.nodes.get_mut(&k) {
            n.prev = None;
            n.next = None;
        }
    }

    /// Puts `k` (in the map, out of the list) at the most recent end.
    fn push_back(&mut self, k: usize) {
        let old = self.tail;
        if let Some(n) = self.nodes.get_mut(&k) {
            n.prev = old;
            n.next = None;
        }
        match old.and_then(|t| self.nodes.get_mut(&t)) {
            Some(t) => t.next = Some(k),
            None => self.head = Some(k),
        }
        self.tail = Some(k);
    }

    /// Takes `k` out of the list and the map.
    fn remove(&mut self, k: usize) -> Option<LruNode> {
        self.unlink(k);
        let node = self.nodes.remove(&k)?;
        self.total = self.total.saturating_sub(node.seen);
        Some(node)
    }

    /// Sets `k`'s `seen` to its meter now.
    fn reread(&mut self, k: usize) {
        if let Some(n) = self.nodes.get_mut(&k) {
            let now = n.file.meter().used();
            self.total = self.total.saturating_sub(n.seen).saturating_add(now);
            n.seen = now;
        }
    }
}

impl DepCache {
    pub(crate) fn new(bound: u64, evicts: bool) -> Self {
        Self {
            bound,
            evicts,
            lru: Mutex::new(Lru::default()),
            loaded: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            #[cfg(test)]
            steps: AtomicU64::new(0),
        }
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn step(&self) {
        #[cfg(test)]
        self.steps.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks `dep` (open as `file`) the most recently used, then evicts the
    /// least recently used others while the sum of the meters passes the
    /// bound (type docs). A new entry's share of the map is charged to
    /// `file`'s meter first; a refusal caches nothing and is the caller's
    /// error.
    pub(crate) fn touch(&self, dep: &Arc<Dependency>, file: &File) -> Result<(), Error> {
        let k = lru_key(dep);
        let known = lock(&self.lru).nodes.contains_key(&k);
        let charge = if known {
            None
        } else {
            Some(file.meter().charge(LRU_SHARE)?)
        };
        let dropped = {
            let mut lru = lock(&self.lru);
            self.step();
            // Out of the list if it is in it; in the map if it is new (a
            // node another thread put there meanwhile is kept, and this
            // one's charge returned).
            lru.unlink(k);
            if let Some(charge) = charge {
                lru.nodes.entry(k).or_insert_with(|| LruNode {
                    _dep: dep.clone(),
                    file: file.clone(),
                    seen: 0,
                    prev: None,
                    next: None,
                    _charge: charge,
                });
            }
            lru.push_back(k);
            lru.reread(k);
            self.evict_over(&mut lru, k)
        };
        self.count_evictions(dropped);
        Ok(())
    }

    /// Re-reads `dep`'s meter after a read through it (the read's parse
    /// charged after the touch), then evicts as [`Self::touch`] does. Does
    /// nothing for a dependency the LRU does not hold.
    pub(crate) fn refresh(&self, dep: &Arc<Dependency>) {
        let k = lru_key(dep);
        let dropped = {
            let mut lru = lock(&self.lru);
            if !lru.nodes.contains_key(&k) {
                return;
            }
            self.step();
            lru.reread(k);
            self.evict_over(&mut lru, k)
        };
        self.count_evictions(dropped);
    }

    /// Evicts from the least recently used end, never `keep`, while the sum
    /// passes the bound; each candidate's meter is re-read first.
    fn evict_over(&self, lru: &mut Lru, keep: usize) -> Vec<LruNode> {
        let mut dropped = Vec::new();
        while lru.total > self.bound && lru.nodes.len() > 1 {
            let Some(h) = lru.head.filter(|&h| h != keep) else {
                break;
            };
            self.step();
            lru.reread(h);
            if lru.total <= self.bound {
                break;
            }
            dropped.extend(lru.remove(h));
        }
        dropped
    }

    /// Drops the LRU's handle on every dependency but `keep` (a pool
    /// refusal's one retry, plan section 3.13).
    pub(crate) fn evict_others(&self, keep: Option<&Arc<Dependency>>) {
        if !self.evicts {
            return;
        }
        let dropped = {
            let mut lru = lock(&self.lru);
            let keep = keep.map(lru_key);
            let all: Vec<usize> = lru.nodes.keys().copied().collect();
            let mut dropped = Vec::new();
            for k in all {
                if Some(k) != keep {
                    self.step();
                    dropped.extend(lru.remove(k));
                }
            }
            dropped
        };
        self.count_evictions(dropped);
    }

    /// Drops the LRU's handle on `dep`, which nothing can reach any more
    /// (its last registry slot went): not an eviction.
    pub(crate) fn forget(&self, dep: &Arc<Dependency>) {
        let gone = lock(&self.lru).remove(lru_key(dep));
        drop(gone);
    }

    fn count_evictions(&self, dropped: Vec<LruNode>) {
        let n = u64::try_from(dropped.len()).unwrap_or(u64::MAX);
        self.evicted.fetch_add(n, Ordering::Relaxed);
        drop(dropped);
    }

    /// The bytes the cached dependencies' meters hold now (a report: every
    /// meter is read).
    pub(crate) fn bytes(&self) -> u64 {
        lock(&self.lru)
            .nodes
            .values()
            .map(|n| n.file.meter().used())
            .fold(0, u64::saturating_add)
    }
}

/// One registry slot, with the charge for its registration (module docs).
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) kind: SlotKind,
    /// Its rank (module docs).
    pub(crate) rank: Rank,
    /// Whether every entry of the nested container it is has been
    /// registered (set after the last one is).
    expanded: AtomicBool,
    /// Whether a parse through the slot was counted (`EnvStats`).
    counted: AtomicBool,
    /// Whether the container preload walked the table of the file the slot
    /// holds (`parse_preload_table` sets `_preload_table = None` when it
    /// ends). Kept on the slot, which outlives an eviction, so a
    /// dependency reopened after one is not walked again, as UnityPy, which
    /// never evicts, walks each file once.
    walked: AtomicBool,
    /// The slot's own charge (its `Arc`, its rank's path), returned when
    /// the last handle on the slot drops.
    _charge: Charge,
}

impl Drop for Slot {
    /// A dependency whose last slot this was can no longer be reached: the
    /// LRU's entry is then its only other holder (module docs).
    fn drop(&mut self) {
        if let SlotKind::Dependency { dep } = &self.kind
            && Arc::strong_count(dep) == 2
            && let Some(cache) = dep.cache.upgrade()
        {
            cache.forget(dep);
        }
    }
}

/// What a slot reaches.
#[derive(Debug)]
pub(crate) enum SlotKind {
    /// A CAB of a file the caller loaded, parsed at registration (`Eager`):
    /// never evicted.
    Ready(Cab),
    /// An entry of a bundle the caller loaded `Lazy`, parsed on first use
    /// through the bundle's own entry cell: never evicted.
    Entry {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
    },
    /// A dependency's CAB, reached from the LRU's handle on its top-level
    /// file along the slot's rank's path, and reopened from its source after
    /// an eviction. Holds no strong handle.
    Dependency {
        /// The dependency.
        dep: Arc<Dependency>,
    },
}

impl Slot {
    /// Whether the slot is a dependency's (`is_dependency` in UnityPy).
    pub(crate) fn is_dependency(&self) -> bool {
        matches!(self.kind, SlotKind::Dependency { .. })
    }

    /// Whether the container preload walked its file's table.
    pub(crate) fn walked(&self) -> bool {
        self.walked.load(Ordering::Acquire)
    }

    /// Marks its file's table walked.
    pub(crate) fn set_walked(&self) {
        self.walked.store(true, Ordering::Release);
    }
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
    /// The generation the last finished container pass started at (0: none).
    built: AtomicU64,
    /// Entries parsed through slots, for `EnvStats`.
    parsed: AtomicU64,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            inner: RwLock::default(),
            loads: AtomicU64::new(0),
            generation: AtomicU64::new(1),
            built: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
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
/// pass's `RuntimeError`), or a nested container's expansion.
#[derive(Clone, Copy, PartialEq, Eq)]
enum By {
    Load,
    Expansion,
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
        self.insert(name, SlotKind::Ready(cab), load, path, meter, By::Load)
    }

    /// Registers entry `dir_index` of a caller's `Lazy` bundle, of load
    /// `load` at `path`.
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
        self.insert(name, kind, load, path, meter, By::Load)
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
        let kind = SlotKind::Dependency { dep: dep.clone() };
        self.insert(name, kind, load, path, meter, By::Load)
    }

    /// Registers `kind` at rank `(load, path)` under `simplify_name(name)`
    /// with dict semantics by rank (module docs): a new key takes its place
    /// in the order and pays its share of the tables and a copy of itself; a
    /// known key takes the slot only if its rank is greater than its slot's,
    /// moves to the rank if it is less than its position's, and pays only for
    /// the slot (and its new position's path when it moves). Charged before
    /// anything is allocated; a refusal registers nothing.
    fn insert(
        &self,
        name: &RawStr,
        kind: SlotKind,
        load: u64,
        path: &[usize],
        meter: &Arc<Meter>,
        by: By,
    ) -> Result<(), Error> {
        // UnityPy environment.py:252-264
        let lowered = simplify_name(name, meter)?;
        let rank_heap = Rank::heap(path.len());
        let mut charge = Charge::zero(meter);
        charge.grow(SLOT_ARC)?;
        charge.grow(rank_heap)?;
        let rank = Rank {
            load,
            path: Arc::from(path),
        };
        let slot = Arc::new(Slot {
            kind,
            rank: rank.clone(),
            expanded: AtomicBool::new(false),
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
                let slot = if replaces {
                    std::mem::replace(&mut e.slot, slot)
                } else {
                    slot
                };
                (Some((slot, charge)), moves || replaces, false)
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
        // An expansion registers what an eager load registered at load,
        // before any container pass: only a load's change starts a new one.
        if changed && by == By::Load {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        if new_key && by == By::Load {
            LOAD_KEYS_HERE.with(|c| c.set(c.get().saturating_add(1)));
        }
        // What the registration replaced or did not use (and its charges)
        // is dropped after the lock.
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
    /// (no copy when `name` is already simplified) and dropped on return.
    pub(crate) fn slot(
        &self,
        name: &RawStr,
        meter: &Arc<Meter>,
    ) -> Result<Option<Arc<Slot>>, Error> {
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
    /// its source after an eviction); `None` for a nested container, whose
    /// entries are registered then (module docs). A dependency read the pool
    /// refuses evicts the others and retries once.
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
                self.answer(slot, file, Some((bundle, *dir_index)), None)
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
        let cab = self.answer(slot, file, at, Some(dep))?;
        Ok(cab.map(|cab| HeldCab {
            cab,
            hold: Some(top),
        }))
    }

    /// The CAB a slot's file is: a `SerializedFile`, or a raw reader (read
    /// through its bundle when it is an entry); a nested container is none,
    /// and its entries are registered then, as the slot's own kind registers
    /// them (`Lazy` entries of the nested bundle, or dependency slots one
    /// level deeper), at the ranks below the slot's.
    fn answer(
        &self,
        slot: &Arc<Slot>,
        file: File,
        at: Option<(&Arc<BundleFile>, usize)>,
        dep: Option<&Arc<Dependency>>,
    ) -> Result<Option<Cab>, Error> {
        match file {
            File::Serialized(sf) => Ok(Some(Cab::Serialized(sf))),
            File::Raw(raw) => Ok(Some(Cab::Raw(match at {
                Some((bundle, dir_index)) => RawCab::InBundle {
                    bundle: bundle.clone(),
                    dir_index,
                },
                None => RawCab::File(raw),
            }))),
            File::Bundle(nested) => {
                self.expand(slot, &nested, dep)?;
                Ok(None)
            }
            File::Web(_) => Ok(None),
        }
    }

    /// Registers a nested container's entries from its slot, at the ranks
    /// below the slot's, unless they all already are (module docs: every
    /// caller that finds them not yet all registered registers them, and the
    /// second registration of an entry changes nothing).
    fn expand(
        &self,
        slot: &Arc<Slot>,
        nested: &Arc<BundleFile>,
        dep: Option<&Arc<Dependency>>,
    ) -> Result<(), Error> {
        if slot.expanded.load(Ordering::Acquire) {
            return Ok(());
        }
        let meter = nested.meter();
        let load = slot.rank.load;
        let mut path = slot.rank.path().to_vec();
        for (j, d) in nested.directory().iter().enumerate() {
            path.push(j);
            let kind = match dep {
                Some(dep) => SlotKind::Dependency { dep: dep.clone() },
                None => SlotKind::Entry {
                    bundle: nested.clone(),
                    dir_index: j,
                },
            };
            let r = self.insert(&d.path, kind, load, &path, meter, By::Expansion);
            path.pop();
            // A refused registration is asked again by the next `get`.
            r?;
        }
        slot.expanded.store(true, Ordering::Release);
        Ok(())
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
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::DepCache;
    use crate::env::{EnvOptions, Environment, Source};
    use crate::raw::RawStr;
    use bytes::Bytes;

    fn bundle_with(cab: &str) -> Vec<u8> {
        use unityrs_testkit::env::{Obj, bundle, serialized};
        bundle(&[(cab, serialized(&[], &[Obj::named(1, cab, &[])]))])
    }

    /// The LRU's work per use does not grow with the dependencies it holds
    /// (review round 1: `touch` scanned and summed every entry, so `n`
    /// lookups over `n` dependencies cost `n`²). Counted in entries
    /// visited, never by a clock.
    #[test]
    fn the_lru_visits_a_constant_number_of_entries_per_use() {
        let steps = |n: usize| {
            let env = Environment::new(EnvOptions::batch(Arc::new(
                crate::options::FileContext::unitypy(),
            )));
            for i in 0..n {
                let name = format!("CAB-{i}");
                env.load_dependency(Source::Bytes {
                    name: RawStr::from(format!("{name}.bundle").as_str()),
                    data: Bytes::from(bundle_with(&name)),
                })
                .unwrap();
            }
            let cache: &DepCache = &env.inner.cache;
            let before = cache.steps.load(Ordering::Relaxed);
            for i in 0..n {
                let name = RawStr::from(format!("cab-{i}").as_str());
                assert!(
                    env.get_cab(&name, &crate::limits::Meter::unlimited())
                        .unwrap()
                        .is_some()
                );
            }
            cache.steps.load(Ordering::Relaxed) - before
        };
        let (small, large) = (steps(200), steps(800));
        // A lookup touches its dependency and refreshes it: two visits.
        assert!(
            large <= 2 * 800 && small <= 2 * 200,
            "visits: {small} for 200 lookups over 200 dependencies, {large} for 800 over 800"
        );
    }
}
