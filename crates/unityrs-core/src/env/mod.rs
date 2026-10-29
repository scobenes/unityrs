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
//! which is a container, the two modes give identical results. Elsewhere they
//! differ in exactly four ways (plan section 3.13): (a) a failing entry
//! fails the load under `Eager` (with the partial registration above) but
//! only the `get` that parses it under `Lazy` (an entry shadowed by a later
//! one of its path is then never parsed; [`crate::files::BundleFile::load_all`]
//! reproduces `Eager`'s error); (b) a nested container's entries are
//! registered when it is first asked for, at the end of the registry order,
//! not in place, and its own slot answers `None`, where `Eager` answers
//! `None` too unless an earlier directory entry of the same path was a CAB,
//! which `Eager` keeps under the key; (c) a block that fails to decode, or
//! trips a limit, after the last byte any read needs fails the load under
//! `Eager` (UnityPy decodes every block before any entry,
//! `files/BundleFile.py:173-186`) and nothing under `Lazy`; (d) a directory
//! whose spans nest or overlap makes `Lazy` decode and keep the shared
//! blocks once per span, so it can stop at the input's meter
//! ([`crate::LimitKind::InputMemory`]) or decode-work bound
//! ([`crate::LimitKind::DecodeWork`]) where `Eager`'s one run fits.
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
//! [`Source::Bytes`] dependency keeps its bytes, which are the caller's. The files a caller loaded are never evicted. UnityPy keeps
//! everything, which is [`EnvOptions::unitypy`]. A dependency read the pool
//! refuses evicts and retries once; the refusal is never cached
//! ([`crate::once::get_or_publish_result`]), so a later read with room in
//! the pool succeeds.
//!
//! **Locks.** The registry, the `files` list and the LRU are locked only to
//! look up or publish; a file is loaded, an entry parsed and a resolver,
//! fetcher or provider called with no lock held ([`crate::once`]). The one
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

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

pub mod pptr;
pub mod registry;
pub mod resource;

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use bytes::Bytes;

pub use pptr::{DerefError, PPtr};
pub use registry::{Cab, Keys, RawCab, Registry};
pub use resource::{Candidate, SUFFIXES, resource_candidates};

use crate::budget::DecodeBuf;
use crate::container::ContainerRow;
use crate::error::{Error, LimitError, LimitKind};
use crate::files::{File, SerializedFile};
use crate::limits::{Limits, Meter};
use crate::object::ObjectRef;
use crate::options::FileContext;
use crate::raw::RawStr;

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
    /// The dependency LRU's bound, in bytes of file data.
    pub dep_cache_bytes: u64,
    /// The `find_file` fallback; `None` finds nothing.
    pub resolver: Option<Arc<dyn CabResolver>>,
}

impl EnvOptions {
    /// UnityPy's behaviour: eager, nothing evicted, no resolver.
    #[must_use]
    pub fn unitypy() -> Self {
        Self {
            context: Arc::new(FileContext::unitypy()),
            load: LoadMode::Eager,
            dep_cache_bytes: u64::MAX,
            resolver: None,
        }
    }

    /// The batch engine's: lazy, a 512 MiB dependency cache.
    #[must_use]
    pub fn batch(context: Arc<FileContext>) -> Self {
        Self {
            context,
            load: LoadMode::Lazy,
            dep_cache_bytes: 512 << 20,
            resolver: None,
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
}

/// The iterator [`Environment::objects`] returns: a handle per nesting
/// level (the file list position, the bundle, the entry, the object index).
pub struct Objects<'a> {
    env: &'a Environment,
}

impl Iterator for Objects<'_> {
    type Item = Result<ObjectRef, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
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
}

impl Iterator for ContainerItems<'_> {
    type Item = Result<ContainerItem, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

impl fmt::Debug for ContainerItems<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContainerItems").finish_non_exhaustive()
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

struct EnvInner {
    options: EnvOptions,
    files: RwLock<Vec<(RawStr, File)>>,
    registry: Registry,
    stats: RwLock<EnvStats>,
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

impl Environment {
    /// An empty environment.
    #[must_use]
    pub fn new(options: EnvOptions) -> Self {
        Self {
            inner: Arc::new(EnvInner {
                options,
                files: RwLock::new(Vec::new()),
                registry: Registry::default(),
                stats: RwLock::new(EnvStats::default()),
            }),
        }
    }

    /// Its options.
    pub fn options(&self) -> &EnvOptions {
        &self.inner.options
    }

    /// The CAB registry.
    pub fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    /// `load_file`: detect, parse, register the CABs, add to `files` under the
    /// source's name (a repeated name replaces the value in place, as a dict).
    pub fn load(&self, source: Source) -> Result<File, Error> {
        todo!()
    }

    /// Loads `source` as a dependency, as `find_file` loads one through the
    /// resolver: its own meter under [`crate::budget::Reservation::Dependency`]
    /// (so a refusal names the dependency pool), its CABs registered (lazily
    /// under [`LoadMode::Lazy`]) without a strong handle, kept in the LRU and
    /// evictable, and not added to [`Self::files`] (UnityPy has no such call;
    /// a batch engine registers its inputs this way). The returned handle is
    /// the caller's: the dependency is not evicted while the caller holds it.
    pub fn load_dependency(&self, source: Source) -> Result<File, Error> {
        todo!()
    }

    /// `files`, in insertion order: one entry per caller load (not a count
    /// the input chooses).
    pub fn files(&self) -> Vec<(RawStr, File)> {
        todo!()
    }

    /// `get_cab`: the registry under `simplify_name(name)`, an entry parsed
    /// now if it was registered lazily. The lowered key a lookup builds is
    /// charged to `meter` while the lookup runs (the meter of the file whose
    /// input chose `name`; a caller's own string may use
    /// [`Meter::unlimited`]), and a name already in its simplified form is
    /// not copied ([`Registry::get`]).
    pub fn get_cab(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        todo!()
    }

    /// `find_file(name, is_dependency=True)`: [`Self::get_cab`], else the
    /// resolver, loading what it returns as a dependency. `meter` as for
    /// [`Self::get_cab`].
    pub fn find_file(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        todo!()
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
        todo!()
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
        nodes: Option<&Arc<crate::typetree::Tree>>,
    ) -> Result<Option<crate::provider::TreeRef>, crate::walk::ParseError> {
        todo!()
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
    /// The answer is the same entry's either way. `path` is the object's
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
        todo!()
    }

    /// `objects`: every object of every non-dependency file, in load order,
    /// as an iterator (module docs: no allocation per object). A file that
    /// fails to load or parse on the way yields its error.
    pub fn objects(&self) -> Objects<'_> {
        todo!()
    }

    /// `container`: every non-dependency CAB's container rows, CABs in
    /// registry order, after the preload index is built (`environment.py:208-227`),
    /// as an iterator.
    pub fn container(&self) -> Result<ContainerItems<'_>, Error> {
        todo!()
    }

    /// Counters.
    pub fn stats(&self) -> EnvStats {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use super::{Cab, Environment, Registry, read_path};
    use crate::budget::OWNER_COST;
    use crate::budget::{Reservation, Unbounded};
    use crate::error::{Error, LimitKind};
    use crate::limits::{Limits, Meter, heap_cost};

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
