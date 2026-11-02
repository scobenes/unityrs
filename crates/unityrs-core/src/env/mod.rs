// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/files/SerializedFile.py, UnityPy/files/BundleFile.py, UnityPy/files/File.py, UnityPy/helpers/ResourceReader.py. See THIRD_PARTY_LICENSES.
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
//! pinned by a test in `tests/env/modes.rs`): (a) a failing entry fails the load
//! under `Eager` (with the partial registration above) but only the `get`
//! that parses it under `Lazy` (an entry shadowed by a later one of its path
//! is then never parsed; [`crate::files::BundleFile::load_all`] reproduces
//! `Eager`'s error); (b) a nested container's own name is a key under
//! `Lazy`, whose slot answers `None`, where `Eager` registers no such key
//! (and keeps an earlier CAB of that path under it); its entries are
//! registered when it is *reached*, before any lookup that resolves its
//! slot, starts from a file inside it, names it as the asking file's
//! sibling or resource candidate, or walks the container, and before a
//! registration drops its slot (an evicted dependency's before the next
//! container pass), with the position and the value an eager load gives
//! them, so only a lookup by name alone of one of its entries can miss
//! before then ([`registry`]'s module docs state the invariant);
//! (c) a block that fails to decode, or trips a limit,
//! after the last byte any read needs fails the load under `Eager` (UnityPy
//! decodes every block before any entry, `files/BundleFile.py:173-186`) and
//! nothing under `Lazy`; (d) a directory whose spans nest or overlap makes
//! `Lazy` decode and keep the shared blocks once per span, so it can stop
//! at the input's meter ([`crate::LimitKind::InputMemory`]) or decode-work
//! bound ([`crate::LimitKind::DecodeWork`]) where `Eager`'s one run fits.
//!
//! **Threads** (plan section 3.16, item 6). Each key's registration, each
//! `files` entry and each `path_dict` entry is published whole, so no call
//! sees one half made. A load's or a reach's set of keys is not published at
//! once: another thread may find one key of a load, or of a nested
//! container being reached, and miss a later one (review round 6: these
//! docs said concurrent calls act as if made one after another, which that
//! does not). A call that reaches a nested container returns only after its
//! whole subtree is registered, and a load only after all its keys and its
//! `files` entry are. A call's result depends on what was done before it,
//! by its thread or another, as a Python dict's does: a load, a resolver
//! load, and under `Lazy` the first call that reaches a nested container.
//! What a set of calls leaves does not depend on the order of the ones that
//! are not loads: the registry orders entries by rank ([`registry`]), and
//! each `path_dict` entry keeps the least stamp ([`crate::container`]).
//!
//! **Memory.** Each load makes its input's meter
//! ([`crate::limits::Meter::for_input`]): [`crate::budget::Reservation::Input`]
//! for what a caller loads, [`crate::budget::Reservation::Dependency`] for a
//! dependency. Every [`Source`] is bounded by
//! [`crate::limits::Limits::max_input_bytes`] before its bytes are detected
//! ([`LimitKind::InputBytes`]): a [`Source::Path`] is read by [`read_path`],
//! refused from its length before anything is read and charged to that
//! meter before its bytes exist; a [`Source::Fetch`] is refused from the
//! size its fetcher states ([`Fetch::size_hint`]) before it is fetched, and
//! from the length of what it returns; [`Source::Bytes`] from their length.
//! Each registry slot, its key and its share of the
//! registry's tables are charged to the meter of the file that registered
//! it ([`registry`]), and each `files` entry, its share of the list and its
//! lowered key to the loaded file's ([`Environment::files`]); the LRU's
//! evictions allocate nothing ([`registry`]). Listings ([`Environment::objects`],
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
//! The files a caller loaded are never evicted. Every call that reads or
//! registers through a dependency's meter (a load, a lookup, a dereference,
//! a resource read, an object's tree with the script hook, the container
//! pass), when the pool refuses it, evicts the others and retries once, by
//! one rule however the calls nest ([`registry`]'s `with_room`;
//! `tests/env/calls.rs` lists every public call and how it meets the
//! pool); the refusal is never cached
//! ([`crate::once::get_or_publish_result`]), so a later read with room in
//! the pool succeeds. A read the caller makes itself on a dependency's
//! handle (an object's value) is no call of the Environment's, and its
//! refusal is the caller's.
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
//! - `find_file` loads a file **as a dependency** (its `is_dependency`
//!   defaults to true, and a dereference takes the default), keyed by the
//!   path it resolved (`env.path` joined with the name: `load_file` keys a
//!   path by itself); `env.objects` and `env.container` skip it, and since
//!   `env.container` walks `env.cabs`, where a dependency's CAB replaces a
//!   CAB of the same key, the replaced file's rows leave the container while
//!   its objects stay in `env.objects`, which walks `env.files`. With
//!   `is_dependency=False` the file is a caller's load under that key. A raw
//!   reader carries no flag (`parse_file` returns the reader itself), so it
//!   is no dependency to `getattr(f, "is_dependency", False)`.
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
//! ([`crate::once`]; `tests/env/locks.rs` runs loads on another thread while a
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
mod deref;
#[cfg(test)]
mod gate;
mod held;
mod listing;
mod load;
mod object_tree;
#[cfg(test)]
mod preload_tests;
mod resource_data;
#[cfg(test)]
mod threads_tests;
mod watch;

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
use crate::error::{Error, LimitError, LimitKind};
use crate::files::File;
use crate::limits::{Limits, Meter};
use crate::options::FileContext;
use crate::raw::RawStr;
use listing::Files;
use registry::DepCache;

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
    /// The file's bytes. A dependency's fetcher (one a
    /// [`Environment::load_dependency`], a resolver's load as a dependency or
    /// a reopen after an eviction calls) that takes them from the dependency
    /// pool may refuse with the pool's refusal ([`Error::Budget`] naming
    /// [`crate::budget::Reservation::Dependency`]): the Environment then
    /// evicts the other dependencies and fetches once more, as for a
    /// refusal of its own (plan section 3.13), whatever the fetcher did
    /// before it returned the refusal: a refusal it passes on from a call of
    /// its own into the Environment, which that call already retried, is
    /// fetched once more too, since the core cannot tell it from the
    /// fetcher's own (`registry::from_callers`). A caller's
    /// [`Environment::load`] is no dependency's, and returns such a refusal
    /// as it is.
    fn fetch(&self) -> Result<Bytes, Error>;
    /// Its size, when known without fetching. A size above
    /// [`Limits::max_input_bytes`] refuses the source before it is fetched
    /// ([`LimitKind::InputBytes`], the size stated as its value), whatever
    /// the fetch would return; the bytes a fetch returns are checked against
    /// the bound whatever was stated (the [`Source`] docs).
    fn size_hint(&self) -> Option<u64> {
        None
    }
}

/// Something to load. Every kind is bounded by [`Limits::max_input_bytes`]:
/// bytes longer than it, however they came, are refused
/// ([`LimitKind::InputBytes`]) before anything detects, parses or keeps
/// them, so a refused fetch's bytes drop at once, with any charge they
/// carry.
#[derive(Clone, Debug)]
pub enum Source {
    /// Bytes in memory, under a name: refused above the bound from their
    /// length.
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
    /// Bytes fetched on demand: refused above the bound from the size the
    /// fetcher states ([`Fetch::size_hint`]) before it is fetched, and from
    /// the length of the bytes it returns.
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
    within_input_bytes(len, limits)?;
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

/// `len`, an input's length or the length its fetcher states for it
/// ([`Fetch::size_hint`]), within [`Limits::max_input_bytes`]; above it,
/// the input's [`LimitKind::InputBytes`] refusal. Every [`Source`] is
/// checked by it before its bytes are detected (`registry::open_source`).
pub(crate) fn within_input_bytes(len: u64, limits: &Limits) -> Result<(), Error> {
    let max = limits.max_input_bytes;
    if len > max {
        return Err(Error::Limit(LimitError {
            kind: LimitKind::InputBytes,
            value: len,
            max,
        }));
    }
    Ok(())
}

/// Finds a file the registry does not hold, where UnityPy looks in
/// `env.path`: `find_file`'s fallback after `get_cab`
/// (`environment.py:310-354`), and each `load_file(name, True)` of a
/// resource read's fallback (`files/SerializedFile.py:330-349`).
pub trait CabResolver: Send + Sync + fmt::Debug {
    /// The source for `name`, or `None` when there is none. `name` is one of
    /// three kinds, each as UnityPy passes it on (review round 9: these docs
    /// named only the first and the third):
    /// - `find_file`'s name ([`Environment::find_file`]): for a dereference,
    ///   the external's path cleaned as `PPtr.deref` cleans it
    ///   ([`crate::pystr::clean_external_path`]: a leading `archive:/`, then
    ///   `assets/`, dropped, the last `/` component kept, lower-cased); for
    ///   a caller's own call, the name the caller gave, as it is. UnityPy
    ///   opens `env.path` joined with it when that exists; else it takes,
    ///   from the files a walk of `env.path` lists, the first whose walked
    ///   path is the name, else the first whose walked path ends with it, a
    ///   plain suffix with no separator boundary (beside `sub/ab.assets`, a
    ///   root `xab.assets` answers `ab.assets`), else the first whose
    ///   walked path, its basename lower-cased, ends with the name's
    ///   simplified name, which it opens by that lower-cased path, so on a
    ///   case-sensitive file system only a basename all lower case on disk
    ///   is found that way; a miss raises `FileNotFoundError`. Its test of
    ///   the simplified name alone never matches (it compares a bare name
    ///   with the walk's full paths). `environment.py:326-354`; `PARITY.md`
    ///   section 3's `Environment.find_file` row has the whole account (the
    ///   walk built once, links, prefixes).
    /// - In a resource read's fallback ([`Environment::resource`]), first
    ///   each of the reading file's externals' stored path, as it is, not
    ///   cleaned (`archive:/CAB-x/CAB-x`, `Assets/Level/Thing.asset`:
    ///   `files/SerializedFile.py:339-340`),
    /// - then each of its four resource candidates ([`resource_candidates`]:
    ///   the resource path's basename and the three names made from its
    ///   stem, `helpers/ResourceReader.py:11-17`, `SerializedFile.py:342-349`).
    ///
    /// UnityPy's `load_file` opens a name of either of the last two kinds as
    /// a path when one exists there; else, a relative one, joined to
    /// `env.path`; else the case-insensitive match `find_sensitive_path`
    /// finds under `env.path`; else nothing (`environment.py:110-137`).
    fn resolve(&self, name: &[u8]) -> Option<Source>;

    /// What `find_file` gets for `name` (the first kind above), where the
    /// registry has no CAB of it: [`Self::resolve`]'s source by default, a
    /// miss `None`. UnityPy's walk answers a miss two ways
    /// (`environment.py:330-354`): `None` where `load_file` cannot open the
    /// path it matched (a broken link, or a basename it lower-cased, which
    /// on a case-sensitive file system names no file), and
    /// `FileNotFoundError` where it matched nothing. A resolver that tells
    /// them apart (the bindings') returns the second as
    /// `Err(Error::NotFound(NotFound::Cab(name)))`
    /// ([`crate::error::NotFound::Cab`]): [`Environment::find_file`] returns
    /// it as it is, and a dereference, which raises `FileNotFoundError`
    /// either way (`classes/PPtr.py:80-85`), fails with
    /// [`DerefError::NotFound`], which a container preload skips as
    /// UnityPy's skips every `FileNotFoundError`. Any other error fails the
    /// call that asked. An error returned here is the caller's code's, a
    /// pool refusal in it counted as it enters the core, as a fetcher's is
    /// ([`Fetch::fetch`]).
    fn find_file(&self, name: &[u8]) -> Result<Option<Source>, Error> {
        Ok(self.resolve(name))
    }
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
    /// Of those, the ones that looked their external up: a walk keeps an
    /// answer until a key its lookup read changes, by any thread
    /// ([`Environment::container`]).
    pub preload_lookups: u64,
}

struct EnvInner {
    options: EnvOptions,
    files: RwLock<Files>,
    registry: Registry,
    cache: Arc<DepCache>,
    loaded: AtomicU64,
    eager_parsed: AtomicU64,
    preload_derefs: AtomicU64,
    preload_lookups: AtomicU64,
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
                preload_lookups: AtomicU64::new(0),
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
            preload_lookups: self.inner.preload_lookups.load(Ordering::Relaxed),
        }
    }
}

impl Environment {
    /// [`registry::with_room`] on this Environment's LRU for the work of one
    /// of its calls (a lookup, a dereference, a resource read: plan section
    /// 3.13), keeping no dependency cached: the handles the call reads
    /// through are held while it runs, by the caller (a dependency's
    /// top-level handle, as [`Self::load_dependency`] says) and by the call
    /// itself (the asking file's container, `resource_data.rs`'s
    /// `top_bundle`), so an eviction drops only the LRU's.
    fn room<T, E: registry::PoolRefusal>(&self, f: impl FnMut() -> Result<T, E>) -> Result<T, E> {
        registry::with_room(Some(&self.inner.cache), None, f)
    }
}

/// Whether the options evict nothing ([`EnvOptions::dep_cache_bytes`]).
fn never_evicts(options: &EnvOptions) -> bool {
    options.keep_resolver_loads && options.dep_cache_bytes == u64::MAX
}

#[cfg(test)]
mod tests;
