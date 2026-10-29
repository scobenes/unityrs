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

mod container_index;
mod listing;
mod load;

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use bytes::Bytes;

pub use container_index::{ContainerItem, ContainerItems};
pub use listing::{FileEntries, FileEntry, Objects};
pub use pptr::{DerefError, PPtr};
pub use registry::{Cab, HeldCab, Keys, RawCab, Registry};
pub use resource::{Candidate, SUFFIXES, resource_candidates};

use crate::budget::DecodeBuf;
use crate::container::View;
use crate::error::{Error, LimitError, LimitKind, NotFound};
use crate::files::{BundleFile, File, ParentKind, SerializedFile};
use crate::limits::{Limits, Meter};
use crate::object::ObjectRef;
use crate::options::FileContext;
use crate::provider::{ScriptIdent, TreeRef, TreeRequest, TreeSource};
use crate::pystr::{clean_external_path, lower_eq, simplify_name};
use crate::raw::RawStr;
use crate::stream::py_read;
use crate::typetree::Tree;
use crate::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
use container_index::Siblings;
use listing::Files;
use registry::{DepCache, Slot, SlotKind};

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

/// What a dereference reached: a registered CAB, or a file (a sibling, a
/// `files` entry, one the resolver loaded).
#[derive(Clone)]
enum Found {
    Cab(Cab),
    File(File),
}

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
            Err(e) => return Err(ParseError::Deref(Arc::new(e))),
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
mod tests;
