//! UnityPy's `Environment` (`environment.py:27-363`): loaded files, the CAB
//! registry, dependency loading, pointer dereference, resource reads and the
//! container.
//!
//! Two load modes (plan section 3.13):
//! - [`LoadMode::Eager`] is UnityPy: a bundle decodes every block, then parses
//!   every entry in directory order and registers each `SerializedFile` and
//!   raw entry as a CAB by [`crate::pystr::simplify_name`] *as it parses it*
//!   (`files/File.py:76-90`). A failing entry fails the load, and
//!   `load_file` raises before the bundle enters `files`
//!   (`environment.py:154-157`), but the CABs of the entries parsed before it
//!   stay registered and resolvable (partial registration: probed on
//!   UnityPy 1.25.2 with a bundle `[good CAB, its .resS, bad CAB]`, after
//!   which `env.files` is empty and `env.cabs` holds the first two).
//! - [`LoadMode::Lazy`] is the batch engine's: a bundle registers its entries'
//!   names at open and parses an entry the first time it is asked for
//!   (computed with no lock held and published once: two threads that ask
//!   at once may both parse, and the first to publish wins).
//!
//! On a scene whose blocks all decode, whose entries all parse and none of
//! which is a container, the two modes give identical results, the
//! container pass's `RuntimeError` included. Elsewhere they differ in
//! exactly four ways (plan section 3.13, `PARITY.md` section 19.3, each
//! pinned by a test in `tests/env.rs`): (a) a failing entry fails the load
//! under `Eager` (with the partial registration above) but only the `get`
//! that parses it under `Lazy` (an entry shadowed by a later one of its path
//! is then never parsed; [`crate::files::BundleFile::load_all`] reproduces
//! `Eager`'s error); (b) a nested container's own name is a key under
//! `Lazy`, whose slot answers `None`, where `Eager` registers no such key
//! (and keeps an earlier CAB of that path under it); its entries are
//! registered when it is first asked for, so a lookup of one before then
//! misses, and once registered they have the position and the value an
//! eager load gives them, whichever nested container was asked for first
//! ([`registry`]); (c) a block that fails to decode, or trips a limit,
//! after the last byte any read needs fails the load under `Eager` (UnityPy
//! decodes every block before any entry, `files/BundleFile.py:173-186`) and
//! nothing under `Lazy`; (d) a directory whose spans nest or overlap makes
//! `Lazy` decode and keep the shared blocks once per span, so it can stop
//! at the input's meter ([`crate::LimitKind::InputMemory`]) or decode-work
//! bound ([`crate::LimitKind::DecodeWork`]) where `Eager`'s one run fits.
//!
//! **Threads.** Concurrent calls act as if made one after another in some
//! order: each change to the registry, the `files` list or a file's
//! `path_dict` is published whole, so no call sees a change half made (a
//! nested container's entries are all registered before its `get`
//! returns). A call's result depends on what was done before it, by its
//! thread or another, as a Python dict's does: a load, a resolver load, and
//! under `Lazy` a nested container's first `get`. What a set of calls leaves
//! does not depend on the order of the ones that are not loads: the
//! registry orders entries by rank ([`registry`]), and each `path_dict`
//! entry keeps the least stamp ([`crate::container`]).
//!
//! **Memory.** Each load makes its input's meter
//! ([`crate::limits::Meter::for_input`]): [`crate::budget::Reservation::Input`]
//! for what a caller loads, [`crate::budget::Reservation::Dependency`] for a
//! dependency. A [`Source::Path`] is read by [`read_path`], bounded by
//! [`crate::limits::Limits::max_input_bytes`] and charged to that meter
//! before its bytes exist. Each registry slot, its key and its share of the
//! registry's tables are charged to the meter of the file that registered
//! it ([`registry`]). Listings ([`Environment::objects`],
//! [`Environment::container`], [`Registry::keys`]) are iterators: they hold
//! a handle per nesting level and allocate nothing per record.
//!
//! **Dependencies** (files loaded to resolve a pointer or a resource, or by
//! [`Environment::load_dependency`]) are kept in an LRU bounded by
//! [`EnvOptions::dep_cache_bytes`] (the sum of their meters). The registry
//! holds no strong handle on a dependency, only what reopens it
//! ([`registry`]), so evicting one drops the LRU's handle and with it the
//! dependency's runs and parsed entries, and the bytes of a source that can
//! be read again ([`Source::Path`], [`Source::Fetch`]) with their charge,
//! once nothing else holds them; the next read opens it again from its
//! source on [`crate::limits::Meter::reopen`] of its meter: the same memory
//! account, and the new open's own work allowance, so eviction churn never
//! exhausts a valid dependency's allowance, and a handle of the old open
//! that someone still holds (a nested bundle a resource read kept) goes on
//! counting against the old open's, which nothing resets. A
//! [`Source::Bytes`] dependency keeps its bytes, which are the caller's.
//! The files a caller loaded are never evicted. A dependency read the pool
//! refuses evicts and retries once; the refusal is never cached
//! ([`crate::once::get_or_publish_result`]), so a later read with room in
//! the pool succeeds.
//!
//! **Resolver loads in `files`** (probed on UnityPy 1.25.2, plan section
//! 3.13). UnityPy keeps every file it loads, those it loads to resolve a
//! pointer or a resource included, in `env.files`, for good:
//! - a resource read's fallback (`SerializedFile.load_dependencies`: the
//!   file's externals and the four candidates) calls
//!   `load_file(name, True)`, whose second positional parameter is
//!   `parent`, so those files are **not** dependencies: keyed by the name
//!   asked, their objects are in `env.objects` and their CABs' rows in
//!   `env.container`, and a second fallback loads the file again and
//!   replaces the entry in place;
//! - `find_file` loads a file **as a dependency**, keyed by the path it
//!   resolved (`env.path` joined with the name: `load_file` keys a path by
//!   itself); `env.objects` and `env.container` skip it, and since
//!   `env.container` walks `env.cabs`, where a dependency's CAB replaces a
//!   CAB of the same key, the replaced file's rows leave the container while
//!   its objects stay in `env.objects`, which walks `env.files`.
//!
//! [`EnvOptions::keep_resolver_loads`] (true in [`EnvOptions::unitypy`]) does
//! exactly that: [`Environment::files`] lists each resolver load as a
//! [`FileEntry`], `is_dependency` as UnityPy sets it, and none is evicted.
//! With it false ([`EnvOptions::batch`]) resolver loads are LRU
//! dependencies, evictable, and in neither `files`, `objects` nor
//! `container`, a `PARITY.md` divergence. [`Environment::load_dependency`],
//! which UnityPy has no call for, never adds to `files`.
//!
//! **Locks.** The registry, the `files` list, the LRU and a file's
//! `path_dict` are locked only to look up or publish; a file is loaded, an
//! entry parsed and a resolver, fetcher or provider called with no lock held
//! ([`crate::once`]; `tests/env.rs` runs loads on another thread while a
//! resolver or a fetch waits for them). The one
//! caller code that may run under them is a [`crate::budget::MemoryBudget`]
//! or [`crate::budget::PermitHome`] (a growth charged while a table is
//! published, a replaced value's charge returned), whose contract forbids
//! blocking and calling the core (plan section 3.16).
//!
//! Where UnityPy reaches the filesystem (`find_file`'s walk of `env.path`,
//! `load_file`'s relative paths), the core asks a [`CabResolver`]: the core
//! itself never opens a path it was not given.
//!
//! Owner: package D. Option constructors are implemented by the scaffold.

pub mod pptr;
pub mod registry;
pub mod resource;

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use bytes::Bytes;

pub use pptr::{DerefError, PPtr};
pub use registry::{Cab, HeldCab, Keys, RawCab, Registry};
pub use resource::{Candidate, SUFFIXES, resource_candidates};

use crate::budget::{DecodeBuf, Reservation};
use crate::container::{Container, ContainerPath, ContainerRow, View};
use crate::error::{Error, LimitError, LimitKind, NotFound, Unsupported};
use crate::files::{BundleFile, File, ParentKind, SerializedFile};
use crate::limits::{Charge, HASH_TABLE_SHARES, Limits, Meter, MeteredVec};
use crate::object::ObjectRef;
use crate::options::FileContext;
use crate::provider::{ScriptIdent, TreeRef, TreeRequest, TreeSource};
use crate::pystr::{clean_external_path, lower_eq, py_lower, simplify_name};
use crate::raw::RawStr;
use crate::stream::py_read;
use crate::typetree::Tree;
use crate::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
use registry::{DepCache, Dependency, Rank, Slot, SlotKind, WeakFile, load_keys_here, open_source};

/// When a container's entries are parsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LoadMode {
    /// At load, every entry, as UnityPy does.
    #[default]
    Eager,
    /// On first use, once.
    Lazy,
}

/// Bytes the caller fetches on demand (a batch engine's mmap or object store).
pub trait Fetch: Send + Sync + fmt::Debug {
    /// The file's bytes.
    fn fetch(&self) -> Result<Bytes, Error>;
    /// Its size, when known without fetching (for budgeting).
    fn size_hint(&self) -> Option<u64> {
        None
    }
}

/// Something to load.
#[derive(Clone, Debug)]
pub enum Source {
    /// Bytes in memory, under a name.
    Bytes {
        /// The name UnityPy would key it by (`stream_name`).
        name: RawStr,
        /// The bytes.
        data: Bytes,
    },
    /// A path the core reads whole into memory, with [`read_path`]: refused
    /// above [`Limits::max_input_bytes`], and charged to the input's meter
    /// before it is read. (Not mapped: a mapping of a file another process
    /// truncates faults, and the core takes no new dependency for it; a
    /// caller who wants a mapping passes [`Source::Bytes`] or
    /// [`Source::Fetch`], whose bytes are the caller's.)
    Path(PathBuf),
    /// Bytes fetched on demand.
    Fetch {
        /// The name.
        name: RawStr,
        /// The fetcher.
        fetch: Arc<dyn Fetch>,
    },
}

/// Reads `path` whole for [`Source::Path`]. A file whose length is above
/// `limits.max_input_bytes` is refused before anything is read
/// ([`LimitKind::InputBytes`]). The buffer is allocated once at the stat'd
/// length, charged to `meter` first, and filled; the end of the file is then
/// checked with a read into a fixed stack buffer, so a file that ends where
/// its length says costs exactly its buffer (`heap_cost(len)` and the owner
/// record, [`crate::budget::OWNER_COST`]), never a growth past it. Only a
/// file that outlasts its stat'd length (one that grows while it is read, a
/// pipe, a device) grows the buffer, charged as it grows, and the read stops
/// at the bound. The returned bytes carry the charge.
pub fn read_path(path: &Path, limits: &Limits, meter: &Arc<Meter>) -> Result<Bytes, Error> {
    let mut file = std::fs::File::open(path)?;
    let max = limits.max_input_bytes;
    let len = file.metadata()?.len();
    if len > max {
        return Err(Error::Limit(LimitError {
            kind: LimitKind::InputBytes,
            value: len,
            max,
        }));
    }
    let mut out = DecodeBuf::new(meter, len, max, LimitKind::InputBytes)?;
    // The stat'd length, into the buffer allocated for it (no growth).
    let stated = usize::try_from(len).unwrap_or(usize::MAX);
    let dst = out.grow_zeroed(stated)?;
    let mut filled = 0;
    while let Some(rest) = dst.get_mut(filled..).filter(|r| !r.is_empty()) {
        match file.read(rest) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    out.truncate(filled);
    // Past it, a stack buffer: the buffer grows only for bytes that exist.
    let mut probe = [0_u8; 8192];
    loop {
        match file.read(&mut probe) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(probe.get(..n).unwrap_or_default())?,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out.into_bytes()?)
}

/// Finds a dependency the registry does not hold: UnityPy's `find_file`
/// fallback after `get_cab` (`environment.py:310-354`).
pub trait CabResolver: Send + Sync + fmt::Debug {
    /// The source for `name` (as the caller gave it: a cleaned external path
    /// or a resource candidate), or `None` when there is none.
    fn resolve(&self, name: &[u8]) -> Option<Source>;
}

/// Environment options.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EnvOptions {
    /// Shared by every file loaded.
    pub context: Arc<FileContext>,
    /// When container entries are parsed.
    pub load: LoadMode,
    /// The dependency LRU's bound, in bytes of file data. `u64::MAX` with
    /// [`Self::keep_resolver_loads`] (as [`Self::unitypy`] sets them) evicts
    /// nothing, not even for a pool refusal, which then fails the read; any
    /// other setting may evict (a pool refusal evicts the others and retries
    /// once), and [`Environment::container_path`] then refuses a
    /// dependency's object.
    pub dep_cache_bytes: u64,
    /// The `find_file` fallback; `None` finds nothing.
    pub resolver: Option<Arc<dyn CabResolver>>,
    /// Whether the files the resolver loads (for `find_file` and a resource
    /// read's fallback) are kept in [`Environment::files`] as UnityPy keeps
    /// them, and never evicted (module docs). False keeps them in the
    /// dependency LRU only, a `PARITY.md` divergence.
    pub keep_resolver_loads: bool,
}

impl EnvOptions {
    /// UnityPy's behaviour: eager, nothing evicted, no resolver, resolver
    /// loads kept in `files` as UnityPy keeps them.
    #[must_use]
    pub fn unitypy() -> Self {
        Self {
            context: Arc::new(FileContext::unitypy()),
            load: LoadMode::Eager,
            dep_cache_bytes: u64::MAX,
            resolver: None,
            keep_resolver_loads: true,
        }
    }

    /// The batch engine's: lazy, a 512 MiB dependency cache, resolver loads
    /// in that cache only (evictable, not in `files`).
    #[must_use]
    pub fn batch(context: Arc<FileContext>) -> Self {
        Self {
            context,
            load: LoadMode::Lazy,
            dep_cache_bytes: 512 << 20,
            resolver: None,
            keep_resolver_loads: false,
        }
    }
}

impl Default for EnvOptions {
    fn default() -> Self {
        Self::unitypy()
    }
}

/// Counters, for the batch report and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EnvStats {
    /// Files loaded by the caller.
    pub loaded: u64,
    /// Entries parsed (eagerly or on first use).
    pub entries_parsed: u64,
    /// Dependencies loaded through the resolver.
    pub dependencies_loaded: u64,
    /// Dependencies evicted from the LRU.
    pub dependencies_evicted: u64,
    /// Bytes the LRU holds now.
    pub dependency_bytes: u64,
    /// Preload table pointers the container passes dereferenced: each index
    /// of a file's table at most once per walk, however many rows' spans
    /// cover it ([`Environment::container`]).
    pub preload_dereferences: u64,
}

/// The iterator [`Environment::objects`] returns: a handle per nesting
/// level (the file list position, the bundle, the entry, the object index).
pub struct Objects<'a> {
    env: &'a Environment,
    next_file: usize,
    bundles: Vec<(Arc<BundleFile>, usize)>,
    current: Option<(Arc<SerializedFile>, usize)>,
}

impl Objects<'_> {
    /// Steps into `file`: a `SerializedFile`'s objects, a bundle's `files`
    /// dict; anything else holds no objects (`environment.py:190-204`).
    fn enter(&mut self, file: File) {
        match file {
            File::Serialized(sf) => self.current = Some((sf, 0)),
            File::Bundle(b) => self.bundles.push((b, 0)),
            File::Raw(_) | File::Web(_) => {}
        }
    }
}

impl Iterator for Objects<'_> {
    type Item = Result<ObjectRef, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((sf, k)) = &mut self.current {
                if let Some(&idx) = sf.object_order().get(*k) {
                    *k += 1;
                    if let Some(o) = ObjectRef::new(sf.clone(), idx) {
                        return Some(Ok(o));
                    }
                    continue;
                }
                self.current = None;
            }
            if let Some((b, j)) = self.bundles.last_mut() {
                let Some(dir_index) = b.files().get(*j).map(|s| s.dir_index) else {
                    self.bundles.pop();
                    continue;
                };
                *j += 1;
                let b = b.clone();
                match b.entry(dir_index) {
                    Ok(f) => self.enter(f),
                    Err(e) => return Some(Err(e)),
                }
                continue;
            }
            let entry = self.env.file_at(self.next_file)?;
            self.next_file += 1;
            // `search`: a dependency file holds no listed objects.
            if !entry.is_dependency {
                self.enter(entry.file);
            }
        }
    }
}

impl fmt::Debug for Objects<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Objects").finish_non_exhaustive()
    }
}

/// The iterator [`Environment::container`] returns, CABs in registry order.
pub struct ContainerItems<'a> {
    env: &'a Environment,
    after: Option<Rank>,
    current: Option<(Arc<SerializedFile>, usize)>,
}

impl Iterator for ContainerItems<'_> {
    type Item = Result<ContainerItem, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((sf, k)) = self.current.take() {
                match sf.container() {
                    Ok(c) => {
                        if let Some(row) = c.rows().get(k) {
                            let row = row.clone();
                            self.current = Some((sf.clone(), k + 1));
                            return Some(Ok(ContainerItem { row, file: sf }));
                        }
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
            // UnityPy environment.py:221-224: every non-dependency CAB.
            let (rank, slot) = self.env.registry().slot_after(self.after.as_ref())?;
            self.after = Some(rank);
            if slot.is_dependency() {
                continue;
            }
            match self.env.registry().resolve(&slot) {
                Ok(Some(Cab::Serialized(sf))) => self.current = Some((sf, 0)),
                Ok(_) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl fmt::Debug for ContainerItems<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContainerItems").finish_non_exhaustive()
    }
}

/// One entry of [`Environment::files`]: UnityPy's `env.files[name]`, with
/// the file's `is_dependency` flag.
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// The key: the source's name for a caller's load, the name asked for a
    /// resource fallback's, the resolved source's name for `find_file`'s
    /// (UnityPy keys the joined path).
    pub name: RawStr,
    /// The file.
    pub file: File,
    /// True for a `find_file` load ([`Environment::objects`] and
    /// [`Environment::container`] skip it), false for every other.
    pub is_dependency: bool,
}

/// The iterator [`Environment::files`] returns.
#[derive(Debug)]
pub struct FileEntries<'a> {
    env: &'a Environment,
    next: usize,
}

const _: () = crate::send_sync::<FileEntries<'static>>();

impl Iterator for FileEntries<'_> {
    type Item = FileEntry;

    fn next(&mut self) -> Option<FileEntry> {
        let entry = self.env.file_at(self.next)?;
        self.next += 1;
        Some(entry)
    }
}

/// A container row with the object it points at, as `env.container` yields.
#[derive(Clone, Debug)]
pub struct ContainerItem {
    /// The row.
    pub row: ContainerRow,
    /// The file whose container it is.
    pub file: Arc<SerializedFile>,
}

/// One `files` entry, with the charge for a resolver load's key and share
/// of the list and its index.
struct Listed {
    entry: FileEntry,
    _charge: Option<Charge>,
}

/// `env.files`: a dict, as a list in insertion order and an index by key,
/// and each key lower-cased to the first position of a key that lowers to
/// it (the scan of `files` a top-level file's pointer makes, rule 4, as one
/// lookup).
#[derive(Default)]
struct Files {
    list: Vec<Listed>,
    index: HashMap<RawStr, usize>,
    lowered: HashMap<RawStr, usize>,
}

/// What one listed resolver load costs the `files` tables: three elements
/// of the list and [`HASH_TABLE_SHARES`] buckets of each index (as a
/// registry slot's share, [`registry`]).
const LISTED_SHARE: u64 = 3 * size_of::<Listed>() as u64
    + 2 * HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<usize>() + 1) as u64;

struct EnvInner {
    options: EnvOptions,
    files: RwLock<Files>,
    registry: Registry,
    cache: Arc<DepCache>,
    loaded: AtomicU64,
    eager_parsed: AtomicU64,
    preload_derefs: AtomicU64,
}

/// UnityPy's `Environment`. A cheap handle: clones share one environment.
#[derive(Clone)]
pub struct Environment {
    inner: Arc<EnvInner>,
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment")
            .field("options", &self.inner.options)
            .finish_non_exhaustive()
    }
}

/// Whose CABs a registration makes: a caller's file (slots that may hold
/// it), or a dependency's (slots that reopen it).
enum Target<'a> {
    Caller,
    Dependency(&'a Arc<Dependency>),
}

/// What a dereference reached: a registered CAB, or a file (a sibling, a
/// `files` entry, one the resolver loaded).
#[derive(Clone)]
enum Found {
    Cab(Cab),
    File(File),
}

/// A container pass's index of each bundle's entry names, lower-cased, to
/// the entry the first of them names (rule 4's scan of a bundle's `files`
/// as one lookup): built once for each bundle the pass meets, charged to the
/// bundle's meter, and dropped with the pass.
#[derive(Default)]
struct Siblings {
    bundles: HashMap<usize, SiblingNames>,
}

struct SiblingNames {
    /// Keeps the bundle, and so the address that keys it, for the pass.
    _bundle: Arc<BundleFile>,
    names: HashMap<RawStr, usize>,
    _charge: Charge,
}

impl Siblings {
    /// The directory index of `bundle`'s first `files` entry whose name
    /// lowers to `clean`.
    fn find(&mut self, bundle: &Arc<BundleFile>, clean: &RawStr) -> Result<Option<usize>, Error> {
        let key = Arc::as_ptr(bundle).addr();
        let entry = match self.bundles.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let meter = bundle.meter();
                let slots = bundle.files();
                let n = u64::try_from(slots.len()).unwrap_or(u64::MAX);
                let charge = meter.charge(
                    HASH_TABLE_SHARES
                        .saturating_mul((size_of::<RawStr>() + size_of::<usize>() + 1) as u64)
                        .saturating_mul(n.saturating_add(1)),
                )?;
                let mut names = HashMap::with_capacity(slots.len());
                for s in slots {
                    names
                        .entry(py_lower(&s.name, meter)?)
                        .or_insert(s.dir_index);
                }
                v.insert(SiblingNames {
                    _bundle: bundle.clone(),
                    names,
                    _charge: charge,
                })
            }
        };
        Ok(entry.names.get(clean.as_bytes()).copied())
    }
}

/// The skip list of a preload walk: `next[i]` leads to the first table
/// index from `i` not yet dereferenced (a disjoint-set forest whose finds
/// compress their paths), so each index is dereferenced once however many
/// rows' spans cover it.
struct Undone(MeteredVec<usize>);

impl Undone {
    fn new(len: usize, meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut next = MeteredVec::with_capacity(meter, len.saturating_add(1))?;
        for i in 0..=len {
            next.push(i)?;
        }
        Ok(Self(next))
    }

    /// The first index from `i` not yet done (the table's length past it).
    fn find(&mut self, i: usize) -> usize {
        let next = self.0.as_mut_slice();
        let mut root = i;
        while let Some(&n) = next.get(root) {
            if n == root {
                break;
            }
            root = n;
        }
        let mut at = i;
        while let Some(n) = next.get_mut(at) {
            let up = *n;
            if up == root {
                break;
            }
            *n = root;
            at = up;
        }
        root
    }

    /// Marks `i` done.
    fn done(&mut self, i: usize) {
        if let Some(n) = self.0.as_mut_slice().get_mut(i) {
            *n = i.saturating_add(1);
        }
    }
}

/// What one table entry of a preload walk costs the walk's map of the
/// file's externals: [`HASH_TABLE_SHARES`] buckets.
const TARGET_SHARE: u64 =
    HASH_TABLE_SHARES * (size_of::<i32>() + size_of::<Option<Found>>() + 1) as u64;

/// The name UnityPy keys a source by: the caller's name, or a path's string.
fn source_name(source: &Source) -> RawStr {
    match source {
        Source::Bytes { name, .. } | Source::Fetch { name, .. } => name.clone(),
        Source::Path(p) => RawStr::from(p.as_os_str().as_encoded_bytes()),
    }
}

/// `ResourceReader._get_resource_data(reader, offset, size)`
/// (`helpers/ResourceReader.py:36-38`): `reader.Position = offset;
/// reader.read_bytes(size)`, a Python slice of the reader's bytes.
fn read_cab(cab: &Cab, offset: u64, size: u64) -> Result<Bytes, Error> {
    match cab {
        Cab::Raw(RawCab::InBundle { bundle, dir_index }) => {
            bundle.entry_range(*dir_index, offset..offset.saturating_add(size))
        }
        Cab::Raw(RawCab::File(raw)) => {
            let size = i64::try_from(size).unwrap_or(i64::MAX);
            let (range, _) = py_read(raw.data.len(), i128::from(offset), size);
            Ok(raw.data.slice(range))
        }
        Cab::Serialized(sf) => Err(not_a_reader(sf.name())),
    }
}

/// A `SerializedFile` asked for resource bytes: it has no `read_bytes`
/// (an `AttributeError` in UnityPy).
fn not_a_reader(name: &RawStr) -> Error {
    Error::Malformed {
        what: "resource",
        detail: format!("{name} is a SerializedFile, not a reader").into(),
    }
}

/// A dereference failure as a file-level error (the container preload's
/// raise): a load failure as it is, any other as [`Error::Deref`].
fn deref_error(e: DerefError) -> Error {
    match e {
        DerefError::Load(e) => e,
        other => Error::Deref(Box::new(other)),
    }
}

/// Whether a read failure is a `ValueError` in UnityPy's object mode, the
/// one exception `generate_monobehaviour_node`'s caller swallows
/// (`files/ObjectReader.py:301-304`): UnityPyBoost raises `EOFError` for a
/// read past the end, and a limit, a refused charge, a tree or a provider
/// failure is the core's own, never swallowed.
fn is_value_error(e: &ParseError) -> bool {
    match e {
        ParseError::CheckRead { .. } => true,
        ParseError::Read(r) => {
            r.kind != ReadErrorKind::OutOfBounds
                && !matches!(
                    r.detail,
                    ReadDetail::Limit(_) | ReadDetail::Charge(_) | ReadDetail::Tree(_)
                )
        }
        _ => false,
    }
}

/// `objects[m_PathID]` of a target file (rule 6).
fn object_in(sf: &Arc<SerializedFile>, path_id: i64) -> Result<ObjectRef, DerefError> {
    sf.object(path_id).ok_or_else(|| DerefError::MissingObject {
        file: sf.name().clone(),
        path_id,
    })
}

impl Environment {
    /// An empty environment.
    #[must_use]
    pub fn new(options: EnvOptions) -> Self {
        let cache = Arc::new(DepCache::new(
            options.dep_cache_bytes,
            !never_evicts(&options),
        ));
        Self {
            inner: Arc::new(EnvInner {
                options,
                files: RwLock::new(Files::default()),
                registry: Registry::default(),
                cache,
                loaded: AtomicU64::new(0),
                eager_parsed: AtomicU64::new(0),
                preload_derefs: AtomicU64::new(0),
            }),
        }
    }

    /// Its options.
    #[must_use]
    pub fn options(&self) -> &EnvOptions {
        &self.inner.options
    }

    /// The CAB registry.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    fn ctx(&self) -> &Arc<FileContext> {
        &self.inner.options.context
    }

    fn files_read(&self) -> std::sync::RwLockReadGuard<'_, Files> {
        self.inner
            .files
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn files_write(&self) -> std::sync::RwLockWriteGuard<'_, Files> {
        self.inner
            .files
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn file_at(&self, i: usize) -> Option<FileEntry> {
        self.files_read().list.get(i).map(|l| l.entry.clone())
    }

    /// `load_file`: detect, parse, register the CABs, add to `files` under the
    /// source's name (a repeated name replaces the value in place, as a dict).
    // The source is handed over (the frozen signature); a caller's load
    // keeps nothing of it but its name.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load(&self, source: Source) -> Result<File, Error> {
        let name = source_name(&source);
        let file = self.load_listed(&name, &source, false)?;
        self.inner.loaded.fetch_add(1, Ordering::Relaxed);
        Ok(file)
    }

    /// A load UnityPy's `load_file` makes (the caller's, or a resource
    /// fallback's under `keep_resolver_loads`): its own input meter, its
    /// CABs registered as a caller's, and a `files` entry under `name`
    /// (charged to the file's meter when the resolver chose it).
    fn load_listed(&self, name: &RawStr, source: &Source, resolved: bool) -> Result<File, Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        // UnityPy environment.py:139-160
        let file = open_source(name, source, &meter, ctx)?;
        self.register_file(name, &file, &Target::Caller)?;
        let key = if resolved {
            RawStr::charged(name.as_bytes(), &meter)?
        } else {
            name.clone()
        };
        self.list(key, file.clone(), false, resolved.then_some(&meter))?;
        Ok(file)
    }

    /// Adds `file` to `files` under `name` with dict semantics. A resolver
    /// load's new key is charged to `meter` (its key's copy rides in the
    /// key; this charges its share of the list and the index).
    fn list(
        &self,
        name: RawStr,
        file: File,
        is_dependency: bool,
        meter: Option<&Arc<Meter>>,
    ) -> Result<(), Error> {
        let charge = meter.map(|m| m.charge(LISTED_SHARE)).transpose()?;
        // A new key's lowered form, for rule 4's lookup: charged to the
        // resolver load's meter, a caller's own name as the caller's.
        let lowered = if self.files_read().index.contains_key(name.as_bytes()) {
            None
        } else {
            Some(match meter {
                Some(m) => py_lower(&name, m)?,
                None => py_lower(&name, &Meter::unlimited())?,
            })
        };
        let replaced = {
            let mut files = self.files_write();
            let at = files.index.get(name.as_bytes()).copied();
            if let Some(entry) = at.and_then(|i| files.list.get_mut(i)) {
                entry.entry.is_dependency = is_dependency;
                Some(std::mem::replace(&mut entry.entry.file, file))
            } else {
                let at = files.list.len();
                files.list.push(Listed {
                    entry: FileEntry {
                        name: name.clone(),
                        file,
                        is_dependency,
                    },
                    _charge: charge,
                });
                files.index.insert(name, at);
                if let Some(lowered) = lowered {
                    files.lowered.entry(lowered).or_insert(at);
                }
                None
            }
        };
        drop(replaced);
        Ok(())
    }

    /// Registers the CABs of a top-level `file` loaded under `name`: the
    /// file itself, or a bundle's entries (`Eager`: as UnityPy parses them;
    /// `Lazy`: every directory entry's name now, parsed on first use).
    /// Every registration is one load's, ranked by its directory path
    /// ([`registry`]).
    fn register_file(&self, name: &RawStr, file: &File, target: &Target<'_>) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let load = reg.next_load();
        match file {
            File::Serialized(sf) => self.register_cab(
                name,
                Cab::Serialized(sf.clone()),
                file.meter(),
                target,
                load,
                &[],
            ),
            File::Raw(raw) => self.register_cab(
                name,
                Cab::Raw(RawCab::File(raw.clone())),
                file.meter(),
                target,
                load,
                &[],
            ),
            File::Bundle(b) => match self.inner.options.load {
                LoadMode::Eager => self.register_eager(b, target, load, &mut Vec::new()),
                LoadMode::Lazy => {
                    for (i, d) in b.directory().iter().enumerate() {
                        match target {
                            Target::Caller => {
                                reg.register_entry(&d.path, b.clone(), i, load, &[i], b.meter())?;
                            }
                            Target::Dependency(dep) => {
                                reg.register_dependency(&d.path, dep, load, &[i], b.meter())?;
                            }
                        }
                    }
                    Ok(())
                }
            },
            File::Web(_) => Err(Error::Unsupported(Unsupported::WebFile)),
        }
    }

    fn register_cab(
        &self,
        name: &RawStr,
        cab: Cab,
        meter: &Arc<Meter>,
        target: &Target<'_>,
        load: u64,
        path: &[usize],
    ) -> Result<(), Error> {
        let reg = &self.inner.registry;
        match target {
            Target::Caller => reg.register_ready(name, cab, load, path, meter),
            Target::Dependency(dep) => reg.register_dependency(name, dep, load, path, meter),
        }
    }

    /// UnityPy's eager bundle: every block decoded, then every entry parsed
    /// in directory order and each `SerializedFile` and raw entry registered
    /// as it is parsed (`files/BundleFile.py:168-186`, `files/File.py:76-90`),
    /// a nested bundle's entries in place. A failing entry fails the bundle
    /// with the CABs before it registered; a block that fails registers
    /// nothing (plan section 3.13). [`BundleFile::load_all`] decodes and
    /// parses first, so this walk only reads its cells: when it failed with
    /// an entry's error, the blocks all decoded and the entries before the
    /// failing one are registered; any other failure is the decode's.
    fn register_eager(
        &self,
        b: &Arc<BundleFile>,
        target: &Target<'_>,
        load: u64,
        path: &mut Vec<usize>,
    ) -> Result<(), Error> {
        let all = b.load_all();
        if let Err(e) = &all
            && !matches!(e, Error::Entry { .. })
        {
            return all;
        }
        for (i, d) in b.directory().iter().enumerate() {
            path.push(i);
            let step = b.entry(i).and_then(|f| {
                self.inner.eager_parsed.fetch_add(1, Ordering::Relaxed);
                match f {
                    File::Bundle(nested) => self.register_eager(&nested, target, load, path),
                    File::Serialized(sf) => self.register_cab(
                        &d.path,
                        Cab::Serialized(sf),
                        b.meter(),
                        target,
                        load,
                        path,
                    ),
                    File::Raw(_) => {
                        let cab = Cab::Raw(RawCab::InBundle {
                            bundle: b.clone(),
                            dir_index: i,
                        });
                        self.register_cab(&d.path, cab, b.meter(), target, load, path)
                    }
                    File::Web(_) => Ok(()),
                }
            });
            path.pop();
            if let Err(e) = step {
                return Err(all.err().unwrap_or(e));
            }
        }
        all
    }

    /// Loads `source` as a dependency, as `find_file` loads one through the
    /// resolver: its own meter under [`crate::budget::Reservation::Dependency`]
    /// (so a refusal names the dependency pool), its CABs registered (lazily
    /// under [`LoadMode::Lazy`]) without a strong handle, kept in the LRU and
    /// evictable, and not added to [`Self::files`] (UnityPy has no such call;
    /// a batch engine registers its inputs this way). The returned handle is
    /// the caller's: an eviction drops only the LRU's, so while the caller
    /// holds it the slots reach the same open file through their weak handle.
    // The source is handed over (the frozen signature) and kept by a clone.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load_dependency(&self, source: Source) -> Result<File, Error> {
        let name = source_name(&source);
        self.load_dep(&name, &source)
    }

    fn load_dep(&self, name: &RawStr, source: &Source) -> Result<File, Error> {
        match self.try_load_dep(name, source) {
            // A pool refusal evicts the others and retries once (plan 3.13);
            // a second refusal fails naming the pool, and nothing of the
            // dependency is kept (its first attempt's charges returned
            // with its handle).
            Err(Error::Budget(b)) if b.what == Reservation::Dependency => {
                self.inner.cache.evict_others(None);
                self.try_load_dep(name, source)
            }
            other => other,
        }
    }

    /// One attempt of [`Self::load_dep`], on a meter of its own.
    fn try_load_dep(&self, name: &RawStr, source: &Source) -> Result<File, Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Dependency);
        let file = open_source(name, source, &meter, ctx)?;
        let dep = Arc::new(Dependency {
            name: name.clone(),
            source: source.clone(),
            meter: Mutex::new(meter),
            open: Mutex::new(WeakFile::of(&file)),
            ctx: ctx.clone(),
            cache: Arc::downgrade(&self.inner.cache),
        });
        self.register_file(&dep.name, &file, &Target::Dependency(&dep))?;
        self.inner.cache.loaded.fetch_add(1, Ordering::Relaxed);
        self.inner.cache.touch(&dep, &file)?;
        // Another load registered every key this one did meanwhile: no slot
        // reaches it, and the LRU lets go of it ([`registry`]).
        if Arc::strong_count(&dep) == 2 {
            self.inner.cache.forget(&dep);
        }
        Ok(file)
    }

    /// `files`, in insertion order: one entry per caller load, and with
    /// [`EnvOptions::keep_resolver_loads`] one per file the resolver loaded
    /// (module docs), a repeated key replacing the value in place. A
    /// resolver load's entry is charged to that file's meter (its key and its
    /// share of the list, as a registry slot's are, [`registry`]), since the
    /// input chooses how many there are. An iterator, as the other listings
    /// are (module docs): it takes the lock briefly per entry and allocates
    /// nothing but the entry it yields.
    #[must_use]
    pub fn files(&self) -> FileEntries<'_> {
        FileEntries { env: self, next: 0 }
    }

    /// `get_cab`: the registry under `simplify_name(name)`, an entry parsed
    /// now if it was registered lazily. The lowered key a lookup builds is
    /// charged to `meter` while the lookup runs (the meter of the file whose
    /// input chose `name`; a caller's own string may use
    /// [`Meter::unlimited`]), and a name already in its simplified form is
    /// not copied ([`Registry::get`]). A dependency's CAB may outlive its
    /// bundle in the LRU: [`Self::get_cab_held`] returns the handle that
    /// keeps it.
    pub fn get_cab(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        self.inner.registry.get(name, meter)
    }

    /// [`Self::get_cab`], with the handle that keeps a dependency's CAB's
    /// container alive while the caller holds it ([`HeldCab`]): an eviction
    /// drops only the LRU's handle, so the CAB's pointers into its sibling
    /// entries keep resolving (rather than failing with
    /// [`DerefError::ContainerDropped`]).
    pub fn get_cab_held(
        &self,
        name: &RawStr,
        meter: &Arc<Meter>,
    ) -> Result<Option<HeldCab>, Error> {
        self.inner.registry.get_held(name, meter)
    }

    /// `find_file(name, is_dependency=True)`: [`Self::get_cab`], else the
    /// resolver, loading what it returns as a dependency: with
    /// [`EnvOptions::keep_resolver_loads`] also a [`FileEntry`] with
    /// `is_dependency` set, under the resolved source's name, never evicted
    /// (module docs). `meter` as for [`Self::get_cab`]. A container the
    /// resolver loaded is no CAB (`None`).
    pub fn find_file(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        Ok(match self.find(name, meter)? {
            Some(Found::Cab(cab)) => Some(cab),
            Some(Found::File(File::Serialized(sf))) => Some(Cab::Serialized(sf)),
            Some(Found::File(File::Raw(raw))) => Some(Cab::Raw(RawCab::File(raw))),
            Some(Found::File(File::Bundle(_) | File::Web(_))) | None => None,
        })
    }

    fn find(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Found>, Error> {
        // UnityPy environment.py:326-329: `get_cab(simplify_name(name))`,
        // which simplifies again.
        let simple = simplify_name(name, meter)?;
        if let Some(cab) = self.inner.registry.get(&simple, meter)? {
            return Ok(Some(Found::Cab(cab)));
        }
        drop(simple);
        // :330-354: the resolver stands for the walk of `env.path`.
        let Some(resolver) = self.inner.options.resolver.clone() else {
            return Ok(None);
        };
        let Some(source) = resolver.resolve(name.as_bytes()) else {
            return Ok(None);
        };
        Ok(Some(Found::File(self.load_resolved(&source, None)?)))
    }

    /// A file the resolver returned: `find_file`'s (`asked` is `None`) is a
    /// dependency, listed in `files` under the source's name with
    /// `keep_resolver_loads`; a resource fallback's (`asked` is the name it
    /// asked for) is a caller's load under that name with
    /// `keep_resolver_loads`, and a dependency without it.
    fn load_resolved(&self, source: &Source, asked: Option<&RawStr>) -> Result<File, Error> {
        let keep = self.inner.options.keep_resolver_loads;
        match asked {
            Some(name) if keep => self.load_listed(name, source, true),
            Some(name) => self.load_dep(name, source),
            None => {
                let name = source_name(source);
                let file = self.load_dep(&name, source)?;
                if keep {
                    let meter = file.meter().clone();
                    let key = RawStr::charged(name.as_bytes(), &meter)?;
                    self.list(key, file.clone(), true, Some(&meter))?;
                }
                Ok(file)
            }
        }
    }

    /// `PPtr.deref` from an object in `from` (`classes/PPtr.py:43-90`), by
    /// the rules of [`pptr`]: `from`'s container is its bundle
    /// ([`crate::files::ParentKind::Bundle`]'s handle), or, for a file loaded
    /// at top level, this Environment, whose [`Self::files`] keys the sibling
    /// scan compares. `from` need not have been loaded here (a batch job's
    /// own bundle handle): its bundle siblings come from its parent, every
    /// other CAB from this Environment. When `from`'s bundle has been
    /// dropped (its weak handle no longer upgrades) the dereference fails
    /// with [`DerefError::ContainerDropped`] rather than guess: UnityPy
    /// cannot reach that state, since an object keeps its file's parent
    /// alive, so the bindings and a batch engine keep a strong bundle handle
    /// beside every `SerializedFile` they hand out.
    pub fn deref(&self, from: &Arc<SerializedFile>, pptr: PPtr) -> Result<ObjectRef, DerefError> {
        // UnityPy classes/PPtr.py:49-50
        if pptr.path_id == 0 {
            return Err(DerefError::Null);
        }
        // :53-54
        if pptr.file_id == 0 {
            return object_in(from, pptr.path_id);
        }
        let target = self.external(from, pptr.file_id, None)?;
        Self::object_of(&target, pptr.path_id)
    }

    /// The file `from`'s external `file_id` names (`classes/PPtr.py:57-85`):
    /// a sibling in `from`'s container (the bundle's `files`, or for a
    /// top-level file this Environment's), else `find_file`. `siblings` is a
    /// container pass's index of bundle names; without it the bundle's
    /// names are scanned.
    fn external(
        &self,
        from: &Arc<SerializedFile>,
        file_id: i32,
        siblings: Option<&mut Siblings>,
    ) -> Result<Found, DerefError> {
        // :57-60
        let externals = from.externals();
        let external = match pptr::external_index(file_id, externals.len()) {
            Ok(i) => externals.get(i),
            Err(pptr::ExternalIndexError::PastEnd) => {
                return Err(DerefError::FileIdPastEnd { file_id });
            }
            Err(pptr::ExternalIndexError::BelowStart) => None,
        };
        let Some(external) = external else {
            return Err(DerefError::FileIdBelowStart { file_id });
        };
        let meter = from.meter();
        // :68-73
        let clean = clean_external_path(&external.path, meter).map_err(Error::from)?;
        // :63-78: the container's `files`, each key lower-cased, the first
        // equal one wins.
        let sibling = match from.parent() {
            Some(parent) => match &parent.kind {
                ParentKind::Bundle { bundle, .. } => {
                    let Some(bundle) = bundle.upgrade() else {
                        return Err(DerefError::ContainerDropped {
                            name: parent.name.clone(),
                        });
                    };
                    let hit = match siblings {
                        Some(index) => index.find(&bundle, &clean)?,
                        None => bundle
                            .files()
                            .iter()
                            .find(|s| lower_eq(s.name.as_bytes(), clean.as_bytes()))
                            .map(|s| s.dir_index),
                    };
                    hit.map(|i| bundle.entry(i)).transpose()?
                }
                ParentKind::Web => None,
            },
            None => self.files_match(&clean),
        };
        match sibling {
            Some(f) => Ok(Found::File(f)),
            // :80-85
            None => match self.find(&clean, meter)? {
                Some(t) => Ok(t),
                None => Err(DerefError::NotFound {
                    path: external.path.clone(),
                }),
            },
        }
    }

    /// The first `files` entry whose key lower-cases to `clean` (the
    /// Environment as a top-level file's container, rule 4), by the lowered
    /// index ([`Files`]).
    fn files_match(&self, clean: &RawStr) -> Option<File> {
        let files = self.files_read();
        let at = *files.lowered.get(clean.as_bytes())?;
        files.list.get(at).map(|l| l.entry.file.clone())
    }

    /// The object holding `path_id` in what a dereference reached
    /// (`classes/PPtr.py:90`, `assetsfile_dst.objects[m_PathID]`).
    fn object_of(target: &Found, path_id: i64) -> Result<ObjectRef, DerefError> {
        match target.clone() {
            Found::Cab(Cab::Serialized(sf)) | Found::File(File::Serialized(sf)) => {
                object_in(&sf, path_id)
            }
            Found::Cab(Cab::Raw(RawCab::InBundle { bundle, dir_index })) => {
                Err(DerefError::RawTarget {
                    name: bundle
                        .directory()
                        .get(dir_index)
                        .map(|d| d.path.clone())
                        .unwrap_or_default(),
                })
            }
            Found::Cab(Cab::Raw(RawCab::File(raw))) | Found::File(File::Raw(raw)) => {
                Err(DerefError::RawTarget {
                    name: raw.name.clone(),
                })
            }
            Found::File(File::Bundle(b)) => Err(DerefError::RawTarget {
                name: b.name().clone(),
            }),
            Found::File(File::Web(w)) => Err(DerefError::RawTarget {
                name: w.name().clone(),
            }),
        }
    }

    /// The tree an object reads with, as UnityPy resolves it
    /// (`files/ObjectReader.py:285-307`): [`ObjectRef::tree`], plus the
    /// `MonoBehaviour` script hook. When the tree came from the provider
    /// chain (not `nodes`, not the type record's own: UnityPy calls
    /// `generate_monobehaviour_node` only inside `if not node:`,
    /// `files/ObjectReader.py:296-304`), its root type is `MonoBehaviour`,
    /// and a provider accepts script requests
    /// ([`crate::provider::TypeTreeProvider::accepts_scripts`], UnityPy's
    /// `env.typetree_generator`), the head is read with that tree
    /// (`check_read` off), `m_Script` is dereferenced through this
    /// Environment and parsed, and the provider is asked with its
    /// [`crate::provider::ScriptIdent`]. As in UnityPy only a `ValueError`
    /// there falls back to the class tree (a null `m_Script`, a head read that
    /// fails with a value error, the provider's miss); a dereference failure
    /// (`FileNotFoundError`, `KeyError`, `IndexError`) is the object's error.
    /// With no such provider nothing is dereferenced (UnityPy raises its
    /// `ValueError` before any deref), so in P1 this equals [`ObjectRef::tree`].
    pub fn object_tree(
        &self,
        obj: &ObjectRef,
        nodes: Option<&Arc<Tree>>,
    ) -> Result<Option<TreeRef>, ParseError> {
        let found = obj.tree(nodes)?;
        let Some(class_tree) = found else {
            return Ok(None);
        };
        let chain = &obj.file().context().providers;
        if !matches!(class_tree.source, TreeSource::Provider(_))
            || class_tree.tree.root_type() != b"MonoBehaviour"
            || !chain.accepts_scripts()
        {
            return Ok(Some(class_tree));
        }
        // UnityPy files/ObjectReader.py:332-350
        let Some(ident) = self.script_ident(obj, &class_tree.tree)? else {
            return Ok(Some(class_tree));
        };
        let Some(version) = obj.file().version() else {
            return Ok(Some(class_tree));
        };
        let req = TreeRequest {
            class_id: obj.class_id(),
            version: &version.version,
            script: Some(&ident),
        };
        // :346-350: the generator's miss is a `ValueError`: the class tree.
        Ok(Some(chain.lookup(&req)?.unwrap_or(class_tree)))
    }

    /// `parse_monobehaviour_head` and `m_Script.deref_parse_as_object()`:
    /// the script's identity, or `None` where UnityPy raises a `ValueError`
    /// (the caller falls back to the class tree).
    fn script_ident(
        &self,
        obj: &ObjectRef,
        class_tree: &Arc<Tree>,
    ) -> Result<Option<ScriptIdent>, ParseError> {
        // :325-330: the head, read with the class tree, `check_read` off.
        let head = match obj.read_typetree(Some(class_tree), false) {
            Ok(p) => p,
            Err(e) if is_value_error(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(view) = View::new(&head) else {
            return Ok(None);
        };
        let attr_error = || {
            ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python("AttributeError"),
            })
        };
        let script = view
            .attr(view.root(), b"m_Script")
            .map_err(|_| attr_error())?;
        let pptr = view.pptr(script).map_err(|_| attr_error())?;
        // `deref`: a null pointer is a `ValueError`.
        let target = match self.deref(obj.file(), pptr) {
            Ok(t) => t,
            Err(DerefError::Null) => return Ok(None),
            Err(e) => return Err(ParseError::Deref(Box::new(e))),
        };
        // `parse_as_object()`, `check_read` on.
        let script = match target.read_typetree(None, true) {
            Ok(p) => p,
            Err(e) if is_value_error(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(sview) = View::new(&script) else {
            return Ok(None);
        };
        let field = |name: &[u8]| {
            sview
                .attr(sview.root(), name)
                .ok()
                .and_then(|n| match n.value() {
                    crate::value::Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or_else(attr_error)
        };
        Ok(Some(ScriptIdent {
            assembly: field(b"m_AssemblyName")?,
            namespace: field(b"m_Namespace")?,
            class: field(b"m_ClassName")?,
        }))
    }

    /// `get_resource_data(path, from, offset, size)`
    /// (`helpers/ResourceReader.py:10-38`). The candidate is resolved by
    /// UnityPy's rule (the registry, then dependencies), but each candidate's
    /// slot is first *located* without being parsed. When the core can tell,
    /// without reading anything, that the slot is an entry of `from`'s own
    /// input, the entry is read through `from`'s bundle and the shared slot
    /// is never parsed, so neither the resolution nor the read decodes
    /// anything through the shared handle or charges its pool (plan section
    /// 3.13): the slot's open handle is `from`'s bundle itself, or the slot
    /// is a [`Source::Bytes`] whose `Bytes` are `from`'s bundle's source (the
    /// same allocation and range) at the entry's directory index. A slot whose
    /// source is a [`Source::Path`] or [`Source::Fetch`] and whose handle is
    /// not open (evicted) cannot be compared without opening it again, so it
    /// resolves by UnityPy's rule, through the slot. Which of the two a batch
    /// engine's inputs get is its design (`docs/plans/batch.md`).
    /// The answer is the same entry's either way. The fallback (the file's
    /// externals and the candidates, loaded when no candidate is
    /// registered) loads through the resolver as UnityPy's
    /// `load_dependencies` does: with [`EnvOptions::keep_resolver_loads`]
    /// each file it loads is a non-dependency [`FileEntry`] under the name
    /// asked, whose objects and rows [`Self::objects`] and
    /// [`Self::container`] list, loaded again by a later fallback (module
    /// docs); without it, an LRU dependency. `path` is the object's
    /// string, so the candidates are slices of it
    /// ([`resource::resource_candidates`]), and each whole name and lowered
    /// key a lookup builds is charged to `from`'s meter while it lives.
    pub fn resource(
        &self,
        from: &Arc<SerializedFile>,
        path: &RawStr,
        offset: u64,
        size: u64,
    ) -> Result<Bytes, Error> {
        // UnityPy helpers/ResourceReader.py:11-33
        let candidates = resource_candidates(path);
        if let Some(bytes) = self.resource_try(from, &candidates, offset, size)? {
            return Ok(bytes);
        }
        // :26: `assets_file.load_dependencies(possible_names)`
        // (files/SerializedFile.py:330-349): every external, then every
        // candidate, through `load_file(name, True)`.
        let meter = from.meter();
        for external in from.externals() {
            self.fallback_load(&external.path)?;
        }
        for c in &candidates {
            let name = c.joined(meter)?;
            self.fallback_load(&name)?;
        }
        if let Some(bytes) = self.resource_try(from, &candidates, offset, size)? {
            return Ok(bytes);
        }
        let [basename, ..] = candidates;
        Err(Error::NotFound(NotFound::Resource(basename.stem)))
    }

    /// `load_file(name, True)` for a resource fallback: what the resolver
    /// returns for `name`, if anything.
    fn fallback_load(&self, name: &RawStr) -> Result<(), Error> {
        let Some(resolver) = self.inner.options.resolver.clone() else {
            return Ok(());
        };
        let Some(source) = resolver.resolve(name.as_bytes()) else {
            return Ok(());
        };
        self.load_resolved(&source, Some(name)).map(drop)
    }

    /// The first candidate a registered slot answers, read.
    fn resource_try(
        &self,
        from: &Arc<SerializedFile>,
        candidates: &[Candidate; 4],
        offset: u64,
        size: u64,
    ) -> Result<Option<Bytes>, Error> {
        let meter = from.meter();
        for c in candidates {
            let name = c.joined(meter)?;
            let Some(slot) = self.inner.registry.slot(&name, meter)? else {
                continue;
            };
            drop(name);
            if let Some((bundle, i)) = own_entry(from, &slot) {
                // Read through `from`'s own bundle: the slot is not parsed.
                match bundle.entry(i)? {
                    File::Raw(_) => {
                        return Ok(Some(
                            bundle.entry_range(i, offset..offset.saturating_add(size))?,
                        ));
                    }
                    File::Serialized(sf) => return Err(not_a_reader(sf.name())),
                    File::Bundle(_) | File::Web(_) => continue,
                }
            }
            if let Some(cab) = self.inner.registry.resolve(&slot)? {
                return Ok(Some(read_cab(&cab, offset, size)?));
            }
        }
        Ok(None)
    }

    /// `objects`: every object of every non-dependency file of
    /// [`Self::files`], in its order, as an iterator (module docs: no
    /// allocation per object). A file that fails to load or parse on the way
    /// yields its error.
    #[must_use]
    pub fn objects(&self) -> Objects<'_> {
        Objects {
            env: self,
            next_file: 0,
            bundles: Vec::new(),
            current: None,
        }
    }

    /// `container`: every non-dependency CAB's container rows, CABs in
    /// registry order, after the preload index is built (`environment.py:208-227`),
    /// as an iterator. It walks the registry, as UnityPy walks `env.cabs`, so
    /// a CAB a dependency's replaced under its key is not listed.
    pub fn container(&self) -> Result<ContainerItems<'_>, Error> {
        self.build_container_index()?;
        Ok(ContainerItems {
            env: self,
            after: None,
            current: None,
        })
    }

    /// `obj.container` (`files/ObjectReader.py:199-204`): the container index
    /// built first ([`Self::container`]'s pass, whose failure is this call's),
    /// then the path the object's file's `path_dict` holds for its path id
    /// ([`crate::container`]): a row of the file's own container whose asset
    /// is the object, the last such row winning, else the first row whose
    /// preload span reached it. The paths live in the file the object was
    /// read from, as UnityPy's live in its `assets_file`, so a second handle
    /// a caller opened on the same bytes holds only its own rows'.
    ///
    /// Where a dependency may be evicted (every option but
    /// [`EnvOptions::unitypy`]'s, [`EnvOptions::dep_cache_bytes`]), an
    /// object of a dependency's file is refused with
    /// [`Unsupported::EvictableContainerPath`]: an eviction drops the paths
    /// the preload set in that file and its reopen does not walk again, so
    /// the answer would depend on when the LRU evicted (`PARITY.md` section
    /// 19.3). The preload sets no path in such a file.
    pub fn container_path(&self, obj: &ObjectRef) -> Result<Option<ContainerPath>, Error> {
        let sf = obj.file();
        if self.may_reopen(sf) {
            return Err(Error::Unsupported(Unsupported::EvictableContainerPath));
        }
        self.build_container_index()?;
        sf.container()?.path_of(obj.path_id(), sf.meter())
    }

    /// Whether `sf` is a dependency's file the LRU may evict and the
    /// Environment open again.
    fn may_reopen(&self, sf: &SerializedFile) -> bool {
        !never_evicts(&self.inner.options) && sf.meter().reservation() == Reservation::Dependency
    }

    /// `_build_container_index` (`environment.py:208-216`): once until the
    /// next load's registration, every registered `SerializedFile`'s preload
    /// table, dependencies' included, in registry order. UnityPy sets its
    /// flag before the loop and each registration clears it, so a pass that
    /// fails leaves the index built unless the pass itself registered
    /// something; here a pass marks built the registry generation it started
    /// at, whether it ends or fails, and any load's registration since starts
    /// a new generation. A slot's table is marked walked only when its
    /// walk ends (UnityPy clears `_preload_table` then). A pass whose own
    /// dereferences make a new key fails as UnityPy's loop over
    /// `cabs.values()` does (`RuntimeError: dictionary changed size during
    /// iteration`), at the step after; a key a nested container's entries
    /// make under [`LoadMode::Lazy`] is not one (an eager load made it at
    /// load), nor is one another thread's load makes. Two passes may run at
    /// once: each walks every table not yet marked, and each `path_dict`
    /// entry keeps the least stamp ([`crate::container`]), so both leave
    /// what one after the other would.
    fn build_container_index(&self) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let generation = reg.generation();
        if reg.built(generation) {
            return Ok(());
        }
        let result = self.container_pass(generation);
        reg.publish_built(generation);
        result
    }

    fn container_pass(&self, generation: u64) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let changed = || Error::Malformed {
            what: "container",
            detail: "dictionary changed size during iteration".into(),
        };
        let mut siblings = Siblings::default();
        let mut after: Option<Rank> = None;
        let mut grew = false;
        while let Some((rank, slot)) = reg.slot_after(after.as_ref()) {
            if grew {
                return Err(changed());
            }
            // The dependency's file is held while its table is walked, so
            // an eviction the walk makes cannot drop the CAB's bundle.
            let Some(held) = reg.resolve_held(&slot)? else {
                after = Some(rank);
                continue;
            };
            let Cab::Serialized(sf) = &held.cab else {
                after = Some(rank);
                continue;
            };
            if !slot.walked() {
                let container = sf.container()?;
                let before = load_keys_here();
                self.preload(sf, container, (generation, &rank), &mut siblings)?;
                slot.set_walked();
                grew = load_keys_here() != before;
            }
            after = Some(rank);
            drop(held);
        }
        if grew {
            return Err(changed());
        }
        Ok(())
    }

    /// `ContainerHelper.parse_preload_table`
    /// (`helpers/ContainerHelper.py:31-49`): every row's preload pointers
    /// dereferenced, and each target's path id given the row's path in its
    /// file's `path_dict` unless it has one (`setdefault`);
    /// `FileNotFoundError` and `KeyError` skipped, any other failure raised.
    ///
    /// Linear in the rows, the table and the externals (plan decision 11),
    /// where UnityPy's loop is quadratic (every row walks its whole span):
    /// each table index is dereferenced once ([`Undone`]), at the first row
    /// whose span reaches it, which is the row whose `setdefault` UnityPy
    /// keeps; each external is resolved once ([`Self::external`]), each
    /// bundle's names indexed once per pass ([`Siblings`]), and each row's
    /// path copied once, charged to this file's meter. A repeated
    /// dereference answers as the first did: the one load it can make is
    /// `find_file`'s, which UnityPy registers under the path it found, whose
    /// simplified basename is the name asked, so the next lookup finds it (a
    /// container it loads, which registers other names, raises at its first
    /// element). A resolver that names its source otherwise, which UnityPy's
    /// walk cannot, is loaded once per walk.
    fn preload(
        &self,
        sf: &Arc<SerializedFile>,
        container: &Container,
        at: (u64, &Rank),
        siblings: &mut Siblings,
    ) -> Result<(), Error> {
        if matches!(container.preload_table(), Ok(None)) {
            return Ok(());
        }
        let malformed = |detail: &'static str| Error::Malformed {
            what: "container",
            detail: detail.into(),
        };
        let meter = sf.meter();
        let mut undone: Option<Undone> = None;
        let mut targets: HashMap<i32, Option<Found>> = HashMap::new();
        let mut targets_charge = Charge::zero(meter);
        for (r, row) in container.rows().iter().enumerate() {
            // `start = info.preloadIndex`, `size = info.preloadSize`: a
            // member the AssetInfo lacks raises; one that is no `int` raises
            // here too (PARITY.md section 19.3).
            let (Some(start), Some(size)) = (row.preload_index, row.preload_size) else {
                return Err(malformed("an AssetInfo index is not an int"));
            };
            if start < 0 || size <= 0 {
                continue;
            }
            // `len(self._preload_table)`: a table that is not a list raises.
            let table = container.preload_table()?.unwrap_or_default();
            if start + size > i128::try_from(table.len()).unwrap_or(i128::MAX) {
                continue;
            }
            let (Ok(s), Ok(e)) = (usize::try_from(start), usize::try_from(start + size)) else {
                continue;
            };
            let undone = match &mut undone {
                Some(u) => u,
                None => undone.insert(Undone::new(table.len(), meter)?),
            };
            let mut path: Option<Option<RawStr>> = None;
            let mut j = undone.find(s);
            while j < e {
                undone.done(j);
                // An element that is no pointer raises (PARITY.md 19.3).
                let Some(p) = table.get(j).copied().flatten() else {
                    return Err(malformed("a preload table element is not a pointer"));
                };
                if !p.is_set() {
                    j = undone.find(j.saturating_add(1));
                    continue;
                }
                self.inner.preload_derefs.fetch_add(1, Ordering::Relaxed);
                if let Some(target) =
                    self.preload_target(sf, p, &mut targets, &mut targets_charge, siblings)?
                    && !self.may_reopen(target.file())
                {
                    let tf = target.file();
                    let path = match &path {
                        Some(p) => p.clone(),
                        None => path
                            .insert(
                                row.path
                                    .as_ref()
                                    .map(|p| RawStr::charged(p.as_bytes(), meter))
                                    .transpose()?,
                            )
                            .clone(),
                    };
                    let stamp = (
                        at.0,
                        at.1.clone(),
                        u32::try_from(r).unwrap_or(u32::MAX),
                        u32::try_from(j).unwrap_or(u32::MAX),
                    );
                    tf.container()?
                        .set_default(p.path_id, path.as_ref(), &stamp, tf.meter())?;
                }
                j = undone.find(j.saturating_add(1));
            }
        }
        Ok(())
    }

    /// `pptr.deref()` for a preload element: the object, or `None` where the
    /// preload skips (`FileNotFoundError`, `KeyError`). Each external is
    /// resolved once per walk, in `targets` (charged to `charge`).
    fn preload_target(
        &self,
        sf: &Arc<SerializedFile>,
        p: PPtr,
        targets: &mut HashMap<i32, Option<Found>>,
        charge: &mut Charge,
        siblings: &mut Siblings,
    ) -> Result<Option<ObjectRef>, Error> {
        let found = if p.file_id == 0 {
            Some(Found::File(File::Serialized(sf.clone())))
        } else if let Some(t) = targets.get(&p.file_id) {
            t.clone()
        } else {
            let t = match self.external(sf, p.file_id, Some(siblings)) {
                Ok(t) => Some(t),
                Err(DerefError::FileIdPastEnd { .. } | DerefError::NotFound { .. }) => None,
                Err(e) => return Err(deref_error(e)),
            };
            charge.grow(TARGET_SHARE)?;
            targets.insert(p.file_id, t.clone());
            t
        };
        let Some(found) = found else {
            return Ok(None);
        };
        match Self::object_of(&found, p.path_id) {
            Ok(o) => Ok(Some(o)),
            Err(DerefError::MissingObject { .. }) => Ok(None),
            Err(e) => Err(deref_error(e)),
        }
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> EnvStats {
        let cache = &self.inner.cache;
        EnvStats {
            loaded: self.inner.loaded.load(Ordering::Relaxed),
            entries_parsed: self
                .inner
                .eager_parsed
                .load(Ordering::Relaxed)
                .saturating_add(self.inner.registry.parsed()),
            dependencies_loaded: cache.loaded.load(Ordering::Relaxed),
            dependencies_evicted: cache.evicted.load(Ordering::Relaxed),
            dependency_bytes: cache.bytes(),
            preload_dereferences: self.inner.preload_derefs.load(Ordering::Relaxed),
        }
    }
}

/// Whether the options evict nothing ([`EnvOptions::dep_cache_bytes`]).
fn never_evicts(options: &EnvOptions) -> bool {
    options.keep_resolver_loads && options.dep_cache_bytes == u64::MAX
}

/// When `slot` is an entry of `from`'s own input and the core can tell
/// without reading anything (plan section 3.13), `from`'s bundle and the
/// entry's directory index: the slot's bundle is `from`'s, or the slot is a
/// dependency's top-level entry whose open handle is `from`'s bundle or
/// whose [`Source::Bytes`] are the bytes `from`'s bundle was opened on.
fn own_entry(from: &SerializedFile, slot: &Slot) -> Option<(Arc<BundleFile>, usize)> {
    let ParentKind::Bundle { bundle, .. } = &from.parent()?.kind else {
        return None;
    };
    let fb = bundle.upgrade()?;
    match &slot.kind {
        SlotKind::Entry { bundle, dir_index } if Arc::ptr_eq(bundle, &fb) => Some((fb, *dir_index)),
        SlotKind::Ready(Cab::Raw(RawCab::InBundle { bundle, dir_index }))
            if Arc::ptr_eq(bundle, &fb) =>
        {
            Some((fb, *dir_index))
        }
        SlotKind::Dependency { dep } => {
            let &[i] = slot.rank.path() else {
                return None;
            };
            let same_handle = matches!(dep.alive(), Some(File::Bundle(b)) if Arc::ptr_eq(&b, &fb));
            let same_bytes = fb.parent().is_none()
                && matches!(&dep.source, Source::Bytes { data, .. }
                    if data.as_ptr() == fb.source().as_ptr() && data.len() == fb.source().len());
            (same_handle || same_bytes).then_some((fb, i))
        }
        SlotKind::Entry { .. } | SlotKind::Ready(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use super::{Cab, EnvOptions, Environment, Registry, read_path};
    use crate::budget::OWNER_COST;
    use crate::budget::{Reservation, Unbounded};
    use crate::error::{Error, LimitKind};
    use crate::limits::{Limits, Meter, heap_cost};

    #[test]
    fn unitypy_options_keep_resolver_loads_in_files_and_batch_options_do_not() {
        // Round 5 (probed on UnityPy 1.25.2): UnityPy keeps a resource
        // fallback's loads and `find_file`'s in `env.files` for good; the
        // batch engine's options keep them in the evictable LRU only.
        assert!(EnvOptions::unitypy().keep_resolver_loads);
        assert!(EnvOptions::default().keep_resolver_loads);
        let batch = EnvOptions::batch(Arc::new(crate::options::FileContext::unitypy()));
        assert!(!batch.keep_resolver_loads);
    }

    /// A temporary file, removed when dropped, so a failing test leaves
    /// nothing behind.
    struct TempFile(std::path::PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn temp_file(name: &str, len: usize) -> TempFile {
        let path = std::env::temp_dir().join(format!(
            "unityrs-core-read-path-{}-{name}",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![7_u8; len]).unwrap();
        TempFile(path)
    }

    #[test]
    fn lookups_take_the_names_string_and_a_meter_for_the_key_they_build() {
        // Round 4: `get_cab`, `find_file`, `Registry::get` and `resource`
        // took `&[u8]` and no meter, so the lowered key of a name an input
        // chose (and every resource candidate) could only be built
        // uncharged. Package D implements the bodies.
        use crate::files::SerializedFile;
        use crate::raw::RawStr;
        type Lookup<S> = fn(&S, &RawStr, &Arc<Meter>) -> Result<Option<Cab>, Error>;
        type Resource = fn(
            &Environment,
            &Arc<SerializedFile>,
            &RawStr,
            u64,
            u64,
        ) -> Result<bytes::Bytes, Error>;
        let _: Lookup<Environment> = Environment::get_cab;
        let _: Lookup<Environment> = Environment::find_file;
        let _: Lookup<Registry> = Registry::get;
        let _: Resource = Environment::resource;
    }

    #[test]
    fn a_path_is_refused_above_the_bound_before_it_is_read() {
        let path = temp_file("long", 8192);
        let limits = Limits {
            max_input_bytes: 4096,
            ..Limits::default()
        };
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let e = read_path(&path.0, &limits, &meter).unwrap_err();
        // The stat'd length is refused; a read to the bound would report 4097.
        assert!(
            matches!(&e, Error::Limit(l) if l.kind == LimitKind::InputBytes && l.value == 8192),
            "{e:?}"
        );
        assert_eq!(meter.used(), 0, "nothing was read or charged");
    }

    #[test]
    fn a_path_is_charged_to_the_meter_and_released_with_its_bytes() {
        let path = temp_file("fits", 4096);
        let limits = Limits {
            max_input_bytes: 4096,
            ..Limits::default()
        };
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let bytes = read_path(&path.0, &limits, &meter).unwrap();
        assert_eq!(bytes.len(), 4096);
        assert_eq!(meter.used(), heap_cost(4096) + OWNER_COST);
        drop(bytes);
        assert_eq!(meter.used(), 0);
        // A meter too small for the file refuses before allocating.
        let small = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
        let e = read_path(&path.0, &limits, &small).unwrap_err();
        assert!(
            matches!(&e, Error::Limit(l) if l.kind == LimitKind::InputMemory),
            "{e:?}"
        );
    }

    #[test]
    fn a_path_needs_only_its_own_size_from_the_meter_and_the_budget() {
        // Round 3: the end-of-file check grew the full buffer by 64 KiB, so
        // the old and new blocks were charged together and a file over half
        // the budget (or one byte under max_input_bytes) was refused.
        use crate::budget::Counting;
        let len = 3 << 20;
        let path = temp_file("exact", len);
        let need = heap_cost(u64::try_from(len).unwrap()) + OWNER_COST;
        let limits = Limits {
            max_input_bytes: u64::try_from(len).unwrap() + 1,
            ..Limits::default()
        };
        let pool = Arc::new(Counting::new(need));
        let meter = Meter::new(need, pool.clone(), Reservation::Dependency);
        let bytes = read_path(&path.0, &limits, &meter).unwrap();
        assert_eq!(bytes.len(), len);
        assert_eq!((meter.used(), pool.used()), (need, need));
        drop(bytes);
        assert_eq!(pool.used(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_longer_than_its_length_says_stops_at_the_bound() {
        // A device reports length 0 and never ends.
        let limits = Limits {
            max_input_bytes: 100_000,
            ..Limits::default()
        };
        let meter = Meter::new(1 << 30, Arc::new(Unbounded), Reservation::Input);
        let e = read_path(std::path::Path::new("/dev/zero"), &limits, &meter).unwrap_err();
        assert!(
            matches!(&e, Error::Limit(l) if l.kind == LimitKind::InputBytes),
            "{e:?}"
        );
        assert_eq!(meter.used(), 0);
    }
}
