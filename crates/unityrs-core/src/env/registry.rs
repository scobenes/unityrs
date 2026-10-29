//! The CAB registry: UnityPy's `env.cabs` dict (`environment.py:252-280`).
//!
//! Keys are [`crate::pystr::simplify_name`] of the registered name. As a
//! Python dict: a repeated key replaces the value and keeps the first
//! position, and iteration is in first-insertion order (`env.container`
//! walks it). A lazy bundle registers every directory entry's name at open,
//! in directory order. An entry that turns out to be a container is not a
//! CAB: its slot answers `None` and its own entries are registered then, at
//! the end of the order (plan section 3.13, difference (b); an eager load
//! would instead keep an earlier CAB of the same path under the key). Under
//! eager loading a bundle registers its CABs as it parses them, so a later
//! entry's failure leaves the earlier ones registered (difference (a)).
//!
//! **What a slot holds** (the crate's `Slot`). A file the caller loaded is never
//! evicted, so its slot may hold its handle. A dependency's slot never
//! holds a strong handle on it: it holds what *reopens* the dependency (its
//! name, its [`super::Source`], its meter, and the path of directory indexes
//! from its top-level file to the CAB) and a `Weak` on the handle the
//! dependency LRU keeps, so evicting the dependency (dropping the LRU's
//! handle) frees its runs, its parsed entries, its nested bundles and, for a
//! source that can be read again (`Path`, `Fetch`), its bytes and their
//! charge; the next `get` opens it again from the source on
//! [`crate::limits::Meter::reopen`] of its meter (the same memory account,
//! the new open's own work allowance, plan section 3.13) and walks the path
//! to the CAB. Parsed entries are cached in the bundle handle's own cells
//! ([`crate::files::BundleFile::entry`]), not in the registry, so they go
//! with the handle. A CAB inside a nested bundle of a dependency is slotted
//! by its path (the outer entry's index, then the inner one's), never by a
//! handle on the inner bundle, which would keep it, and the slice of the
//! outer's decoded run it is made from, alive past the eviction.
//!
//! **Cells.** A slot's parse is published with
//! [`crate::once::get_or_publish_result`]: a budget or meter refusal is
//! returned and not kept, so a shared dependency refused once (while other
//! jobs held the pool) is parsed again on the next `get`. A parse has no side
//! effect but its charges: a nested container's entries are registered from
//! the published value, once (the slot records that it did), so a parse that
//! lost a race registers nothing.
//!
//! **Memory.** Every registration is charged to the meter of the file that
//! registers it: the key ([`crate::raw::RawStr::charged`]), the slot's
//! `Arc`, three elements of the `order` vector and four buckets of the
//! `slots` hash table (a bucket is a key, a slot handle and a control byte).
//! A doubling vector holds up to two elements per key at rest and three
//! while it grows (the old buffer and the doubled one both live); a doubling
//! hash table up to 16/7 buckets per key at rest and 24/7 while it grows
//! (the crate's `HASH_TABLE_SHARES`), so the two tables' spare capacity and
//! every growth are paid for, and only each table's fixed part (hashbrown's
//! 16-byte control tail, the allocator's rounding of each allocation) is
//! left, a constant per registry. The slot keeps that
//! charge; a key registered again keeps its first slot's share of the tables
//! and the new slot pays its own `Arc`. A refused charge registers nothing
//! and is the registering load's error.
//!
//! Owner: package D.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use super::Source;
use crate::budget::Reservation;
use crate::detect::check_file_type;
use crate::error::Error;
use crate::files::{BundleFile, File, RawFile, SerializedFile, parse_file};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};
use crate::options::FileContext;
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
    /// The LRU that keeps its open handle.
    pub(crate) cache: Arc<DepCache>,
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
/// a top-level file named `name`.
pub(crate) fn open_source(
    name: &RawStr,
    source: &Source,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
) -> Result<File, Error> {
    let data = match source {
        Source::Bytes { data, .. } => data.clone(),
        Source::Path(p) => super::read_path(p, &ctx.limits, meter)?,
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
            self.cache.touch(self, &f);
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
        self.cache.touch(self, &file);
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
/// (`EnvOptions::dep_cache_bytes`). Locked only to reorder or to take a
/// handle out; a handle it drops is dropped after the lock.
#[derive(Debug)]
pub(crate) struct DepCache {
    bound: u64,
    entries: Mutex<Vec<(Arc<Dependency>, File)>>,
    pub(crate) loaded: AtomicU64,
    pub(crate) evicted: AtomicU64,
}

impl DepCache {
    pub(crate) fn new(bound: u64) -> Self {
        Self {
            bound,
            entries: Mutex::new(Vec::new()),
            loaded: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
        }
    }

    /// Marks `dep` (open as `file`) the most recently used, then evicts the
    /// least recently used others while the sum of the meters passes the
    /// bound.
    pub(crate) fn touch(&self, dep: &Arc<Dependency>, file: &File) {
        let dropped = {
            let mut entries = lock(&self.entries);
            if let Some(at) = entries.iter().position(|(d, _)| Arc::ptr_eq(d, dep)) {
                let e = entries.remove(at);
                entries.push(e);
            } else {
                entries.push((dep.clone(), file.clone()));
            }
            let mut total: u64 = entries
                .iter()
                .map(|(_, f)| f.meter().used())
                .fold(0, u64::saturating_add);
            let mut dropped = Vec::new();
            while total > self.bound && entries.len() > 1 {
                let (d, f) = entries.remove(0);
                total = total.saturating_sub(f.meter().used());
                dropped.push((d, f));
            }
            dropped
        };
        self.count_evictions(dropped);
    }

    /// Drops the LRU's handle on every dependency but `keep` (a pool
    /// refusal's one retry, plan section 3.13).
    pub(crate) fn evict_others(&self, keep: Option<&Arc<Dependency>>) {
        let dropped = {
            let mut entries = lock(&self.entries);
            let all = std::mem::take(&mut *entries);
            let (kept, dropped): (Vec<_>, Vec<_>) = all
                .into_iter()
                .partition(|(d, _)| keep.is_some_and(|k| Arc::ptr_eq(d, k)));
            *entries = kept;
            dropped
        };
        self.count_evictions(dropped);
    }

    fn count_evictions(&self, dropped: Vec<(Arc<Dependency>, File)>) {
        let n = u64::try_from(dropped.len()).unwrap_or(u64::MAX);
        self.evicted.fetch_add(n, Ordering::Relaxed);
        drop(dropped);
    }

    /// The bytes the cached dependencies' meters hold now.
    pub(crate) fn bytes(&self) -> u64 {
        lock(&self.entries)
            .iter()
            .map(|(_, f)| f.meter().used())
            .fold(0, u64::saturating_add)
    }
}

/// One registry slot, with the charge for its registration (module docs).
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) kind: SlotKind,
    /// Whether a nested container's entries were registered from it.
    pub(crate) expanded: AtomicBool,
    /// Whether the container preload ran on its file
    /// (`ContainerHelper.parse_preload_table` clears the table once it ran).
    pub(crate) preloaded: AtomicBool,
    /// Whether a parse through the slot was counted (`EnvStats`).
    counted: AtomicBool,
    /// The slot's own charge (its `Arc`, a dependency's path), returned
    /// when the last handle on the slot drops.
    _charge: Charge,
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
    /// file and reopened from its source after an eviction. Holds no strong
    /// handle.
    Dependency {
        /// The dependency.
        dep: Arc<Dependency>,
        /// The directory indexes from the top-level file to the CAB: empty
        /// for the file itself, `[i]` for entry `i` of a bundle, `[i, j]` for
        /// entry `j` of the bundle that is entry `i`, and so on (at most
        /// [`crate::limits::Limits::max_container_depth`] long, charged with
        /// the slot).
        path: Box<[usize]>,
    },
}

impl Slot {
    /// Whether the slot is a dependency's (`is_dependency` in UnityPy).
    pub(crate) fn is_dependency(&self) -> bool {
        matches!(self.kind, SlotKind::Dependency { .. })
    }
}

/// One key's place: the slot, and the first registration's charge for its
/// share of the two tables (module docs).
#[derive(Debug)]
struct Entry {
    slot: Arc<Slot>,
    _share: Charge,
}

/// The registry.
#[derive(Debug, Default)]
pub struct Registry {
    inner: RwLock<RegistryInner>,
    /// UnityPy's `_container_index_built`: set by the container's preload
    /// pass, cleared by every registration (`environment.py:208-216`, `:264`).
    index_built: AtomicBool,
    /// Entries parsed through slots, for `EnvStats`.
    parsed: AtomicU64,
}

#[derive(Debug, Default)]
struct RegistryInner {
    order: Vec<RawStr>,
    slots: HashMap<RawStr, Entry>,
}

/// What one more key costs the two tables: three elements of `order` and
/// [`HASH_TABLE_SHARES`] buckets of `slots` (module docs).
const SHARE: u64 = 3 * size_of::<RawStr>() as u64
    + HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<Entry>() + 1) as u64;

/// A slot's own allocation: the `Slot` in its `Arc`.
const SLOT_ARC: u64 = heap_cost((size_of::<Slot>() + 2 * size_of::<usize>()) as u64);

impl Registry {
    /// `register_cab(name, cab)` for a file the caller loaded, charged to
    /// `meter` (module docs).
    pub fn register(&self, name: &RawStr, cab: Cab, meter: &Arc<Meter>) -> Result<(), Error> {
        self.insert(name, SlotKind::Ready(cab), meter)
    }

    /// Registers entry `dir_index` of a bundle the caller loaded `Lazy`, to
    /// parse on first use; charged to `meter`.
    pub fn register_lazy(
        &self,
        name: &RawStr,
        bundle: Arc<BundleFile>,
        dir_index: usize,
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        self.insert(name, SlotKind::Entry { bundle, dir_index }, meter)
    }

    /// Registers a dependency's CAB at `path` (module docs), charged to
    /// `meter`, the dependency's.
    pub(crate) fn register_dependency(
        &self,
        name: &RawStr,
        dep: &Arc<Dependency>,
        path: &[usize],
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let mut path_charge = Charge::zero(meter);
        if !path.is_empty() {
            path_charge.grow(heap_cost(
                u64::try_from(size_of_val(path)).unwrap_or(u64::MAX),
            ))?;
        }
        let kind = SlotKind::Dependency {
            dep: dep.clone(),
            path: path.into(),
        };
        self.insert_charged(name, kind, meter, path_charge)
    }

    fn insert(&self, name: &RawStr, kind: SlotKind, meter: &Arc<Meter>) -> Result<(), Error> {
        self.insert_charged(name, kind, meter, Charge::zero(meter))
    }

    /// Registers `kind` under `simplify_name(name)` with dict semantics: a
    /// new key is appended to the order and pays its share of the tables and
    /// a copy of itself; a known key keeps its position and its first
    /// share, and only the new slot is paid for (module docs). Charged
    /// before anything is allocated; a refusal registers nothing.
    fn insert_charged(
        &self,
        name: &RawStr,
        kind: SlotKind,
        meter: &Arc<Meter>,
        mut charge: Charge,
    ) -> Result<(), Error> {
        // UnityPy environment.py:252-264
        let lowered = simplify_name(name, meter)?;
        charge.grow(SLOT_ARC)?;
        let slot = Arc::new(Slot {
            kind,
            expanded: AtomicBool::new(false),
            preloaded: AtomicBool::new(false),
            counted: AtomicBool::new(false),
            _charge: charge,
        });
        let known = self.read().slots.contains_key(lowered.as_bytes());
        // A new key's own copy and its share of the tables, made before the
        // lock: the key holds no input's buffer (module docs).
        let fresh = if known {
            None
        } else {
            let share = meter.charge(SHARE)?;
            Some((RawStr::charged(lowered.as_bytes(), meter)?, share))
        };
        let replaced = {
            let mut inner = self.write();
            if let Some(e) = inner.slots.get_mut(lowered.as_bytes()) {
                Some(std::mem::replace(&mut e.slot, slot))
            } else if let Some((key, share)) = fresh {
                inner.order.reserve(1);
                inner.slots.reserve(1);
                inner.order.push(key.clone());
                inner.slots.insert(
                    key,
                    Entry {
                        slot,
                        _share: share,
                    },
                );
                None
            } else {
                // The key went away between the check and the lock: never,
                // since nothing removes one; register it on the next call.
                return Err(Error::Malformed {
                    what: "registry",
                    detail: "a key vanished".into(),
                });
            }
        };
        self.index_built.store(false, Ordering::Release);
        // The replaced slot (and its charge) is dropped after the lock.
        drop(replaced);
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

    /// The key and slot at position `i` of the order.
    pub(crate) fn slot_at(&self, i: usize) -> Option<(RawStr, Arc<Slot>)> {
        let inner = self.read();
        let key = inner.order.get(i)?.clone();
        let slot = inner.slots.get(key.as_bytes())?.slot.clone();
        Some((key, slot))
    }

    /// Whether the container's preload pass already ran (and marks it run).
    pub(crate) fn take_index_build(&self) -> bool {
        !self.index_built.swap(true, Ordering::AcqRel)
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
        match self.slot(name, meter)? {
            Some(slot) => self.resolve(&slot),
            None => Ok(None),
        }
    }

    /// What a slot answers: its CAB, parsed through its bundle's own cell
    /// (`Lazy`) or reached from its dependency's open handle (reopened from
    /// its source after an eviction); `None` for a nested container, whose
    /// entries are registered then, once, at the end of the order (plan
    /// section 3.13, difference (b)). A dependency read the pool refuses
    /// evicts the others and retries once.
    pub(crate) fn resolve(&self, slot: &Arc<Slot>) -> Result<Option<Cab>, Error> {
        match &slot.kind {
            SlotKind::Ready(cab) => Ok(Some(cab.clone())),
            SlotKind::Entry { bundle, dir_index } => {
                let file = bundle.entry(*dir_index)?;
                self.count(slot);
                self.answer(slot, file, Some((bundle, *dir_index)), None)
            }
            SlotKind::Dependency { dep, path } => {
                let first = self.resolve_dependency(slot, dep, path);
                match first {
                    Err(Error::Budget(b)) if b.what == Reservation::Dependency => {
                        dep.cache.evict_others(Some(dep));
                        self.resolve_dependency(slot, dep, path)
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
        path: &[usize],
    ) -> Result<Option<Cab>, Error> {
        let mut file = dep.handle()?;
        let mut at: Option<(Arc<BundleFile>, usize)> = None;
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
        let at = at.as_ref().map(|(b, i)| (b, *i));
        self.answer(slot, file, at, Some((dep, path)))
    }

    /// The CAB a slot's file is: a `SerializedFile`, or a raw reader (read
    /// through its bundle when it is an entry); a nested container is none,
    /// and its entries are registered once, at the end of the order, as the
    /// slot's own kind registers them (`Lazy` entries of the nested bundle,
    /// or dependency slots one level deeper).
    fn answer(
        &self,
        slot: &Arc<Slot>,
        file: File,
        at: Option<(&Arc<BundleFile>, usize)>,
        dep: Option<(&Arc<Dependency>, &[usize])>,
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

    /// Registers a nested container's entries from its slot, once.
    fn expand(
        &self,
        slot: &Arc<Slot>,
        nested: &Arc<BundleFile>,
        dep: Option<(&Arc<Dependency>, &[usize])>,
    ) -> Result<(), Error> {
        if slot
            .expanded
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        let meter = nested.meter();
        let mut path = dep.map(|(_, p)| p.to_vec()).unwrap_or_default();
        let done = (|| {
            for (j, d) in nested.directory().iter().enumerate() {
                match dep {
                    Some((dep, _)) => {
                        path.push(j);
                        let r = self.register_dependency(&d.path, dep, &path, meter);
                        path.pop();
                        r?;
                    }
                    None => self.register_lazy(&d.path, nested.clone(), j, meter)?,
                }
            }
            Ok(())
        })();
        if done.is_err() {
            // A refused registration is asked again by the next `get`.
            slot.expanded.store(false, Ordering::Release);
        }
        done
    }

    /// Keys in dict order, as an iterator that takes the read lock briefly
    /// per key (no snapshot of the registry).
    #[must_use]
    pub fn keys(&self) -> Keys<'_> {
        Keys {
            registry: self,
            next: 0,
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
    next: usize,
}

impl Iterator for Keys<'_> {
    type Item = RawStr;

    fn next(&mut self) -> Option<RawStr> {
        let key = self.registry.read().order.get(self.next).cloned()?;
        self.next += 1;
        Some(key)
    }
}
