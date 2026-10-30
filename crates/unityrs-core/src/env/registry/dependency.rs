//! A dependency's record ([`Dependency`]): what reopens it after an
//! eviction, and the weak handle on the file the LRU keeps open;
//! [`open_source`], which reads a source, detects it and parses it; and
//! [`with_room`], the one retry every dependency read or registration the
//! pool refuses takes.

use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{DepCache, lock};
use crate::budget::Reservation;
use crate::detect::check_file_type;
use crate::env::{DerefError, Source};
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, RawFile, SerializedFile, parse_file};
use crate::limits::{ChargeError, Meter};
use crate::options::FileContext;
use crate::pystr::is_split_path;
use crate::raw::RawStr;
use crate::typetree::TreeError;
use crate::walk::{ParseError, ReadDetail};

/// A dependency the Environment loaded (through the resolver, or
/// `Environment::load_dependency`): what reopens it, and the handle the LRU
/// keeps while it is cached.
#[derive(Debug)]
pub(crate) struct Dependency {
    /// The name it was loaded under.
    pub(crate) name: RawStr,
    /// Where its bytes come from again after an eviction.
    pub(crate) source: Source,
    /// Its current open: the LRU's handle, weakly, and its meter, under one
    /// lock, so a reopen publishes both in one step and no lock is taken
    /// while another is held (plan section 3.16; review round 6: the meter
    /// had a lock of its own, taken while this one was held).
    pub(crate) open: Mutex<Open>,
    /// The context it is parsed with.
    pub(crate) ctx: Arc<FileContext>,
    /// The LRU that keeps its open handle, weakly: the LRU holds the
    /// dependency, so a strong handle back would keep both alive after the
    /// Environment drops.
    pub(crate) cache: Weak<DepCache>,
    /// How many registry slots reach it ([`DepSlot`]): when the last goes,
    /// nothing can reach it any more and the LRU lets go of it.
    pub(crate) slots: AtomicUsize,
}

/// A dependency's current open ([`Dependency::open`]).
#[derive(Debug)]
pub(crate) struct Open {
    /// The LRU's handle, weakly: an eviction drops the LRU's strong one.
    pub(crate) file: Option<WeakFile>,
    /// The meter of the open. An eviction keeps it; the next open is on its
    /// [`Meter::reopen`], which charges the same memory account and has its
    /// own work allowance, so an old open's handle that someone still holds
    /// keeps counting against the old one (plan section 3.13).
    pub(crate) meter: Arc<Meter>,
}

impl Open {
    /// An open of `file` on `meter`.
    pub(crate) fn of(file: &File, meter: Arc<Meter>) -> Self {
        Self {
            file: WeakFile::of(file),
            meter,
        }
    }

    fn alive(&self) -> Option<File> {
        self.file.as_ref().and_then(WeakFile::upgrade)
    }
}

/// A registry slot's handle on its dependency, counted in
/// [`Dependency::slots`]: made with the slot's kind (before the slot, so
/// a registration refused on the way counts it out again) and dropped with
/// it. The count is decided in one atomic step, so of two slots that drop
/// at once exactly one sees itself the last and makes the LRU let go
/// (review round 2: each tested the dependency's `Arc` count, saw the
/// other's handle, and neither did, which left the dependency in an LRU
/// that never evicts).
#[derive(Debug)]
pub(crate) struct DepSlot(Arc<Dependency>);

impl DepSlot {
    pub(crate) fn new(dep: &Arc<Dependency>) -> Self {
        dep.slots.fetch_add(1, Ordering::AcqRel);
        Self(dep.clone())
    }
}

impl Deref for DepSlot {
    type Target = Arc<Dependency>;

    fn deref(&self) -> &Arc<Dependency> {
        &self.0
    }
}

impl Drop for DepSlot {
    fn drop(&mut self) {
        let last = self.0.slots.fetch_sub(1, Ordering::AcqRel) == 1;
        if let Some(cache) = self.0.cache.upgrade() {
            #[cfg(test)]
            cache.hook(super::Hook::SlotReleased);
            if last {
                cache.forget(&self.0);
            }
        }
    }
}

impl Dependency {
    /// Whether no registry slot reaches it ([`DepSlot`]).
    pub(crate) fn unreachable(&self) -> bool {
        self.slots.load(Ordering::Acquire) == 0
    }
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
/// [`crate::env::read_path`], charged first; a [`Source::Fetch`] from its
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
            crate::env::read_path(p, &ctx.limits, meter)?
        }
        Source::Fetch { fetch, .. } => fetch.fetch()?,
    };
    // UnityPy environment.py:139-154
    let ty = check_file_type(&data, &ctx.limits)?;
    parse_file(name.clone(), data, ty, None, meter, ctx)
}

/// Runs `f`, a read or a registration charged to a dependency's meter (one
/// being loaded when `keep` is `None`); when the dependency pool refuses it
/// ([`pool_refused`]), evicts every cached dependency but `keep` and runs
/// it once more, the second refusal being the caller's (plan section 3.13:
/// "a dependency read the pool refuses evicts and retries once"). Every
/// such read takes this one retry: a dependency's load, a lookup that opens
/// or reads one, and a reach of a dependency's nested container, the
/// container pass's reopen included (review round 7: that reopen had none,
/// so under `LoadMode::Lazy` a pool with room for one open failed every
/// container pass where `Eager` made room and succeeded).
pub(crate) fn with_room<T>(
    cache: Option<&DepCache>,
    keep: Option<&Arc<Dependency>>,
    mut f: impl FnMut() -> Result<T, Error>,
) -> Result<T, Error> {
    match f() {
        Err(e) if pool_refused(&e) => {
            drop(e);
            if let Some(cache) = cache {
                cache.evict_others(keep);
            }
            f()
        }
        other => other,
    }
}

/// Whether `e` is the dependency pool refusing a charge
/// ([`Reservation::Dependency`]), as it is or wrapped by an error that
/// carries one: an entry's, a tree's, an object read's or a load's, the
/// wrappers [`crate::once::CellError::is_transient`] looks through (review
/// round 7: a refusal while a reopened dependency's entry was parsed came
/// back as the entry's error, and the retry, which matched only a bare
/// refusal, missed it).
pub(crate) fn pool_refused(e: &Error) -> bool {
    match e {
        Error::Budget(b) => b.what == Reservation::Dependency,
        Error::Entry { source, .. } => pool_refused(source),
        Error::Tree(t) => tree_refused(t),
        Error::AssetBundle(p) => parse_refused(p),
        Error::Deref(d) => deref_refused(d),
        _ => false,
    }
}

fn charge_refused(c: &ChargeError) -> bool {
    matches!(c, ChargeError::Budget(b) if b.what == Reservation::Dependency)
}

fn tree_refused(t: &TreeError) -> bool {
    matches!(t, TreeError::Charge(c) if charge_refused(c))
}

fn deref_refused(d: &DerefError) -> bool {
    matches!(d, DerefError::Load(e) if pool_refused(e))
}

fn parse_refused(p: &ParseError) -> bool {
    match p {
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Charge(c) => charge_refused(c),
            ReadDetail::Tree(t) => tree_refused(t),
            _ => false,
        },
        ParseError::Tree(t) => tree_refused(t),
        ParseError::Deref(d) => deref_refused(d),
        _ => false,
    }
}

impl Dependency {
    /// [`with_room`] for a read or a registration of this dependency, which
    /// the eviction keeps.
    pub(crate) fn with_room<T>(
        self: &Arc<Self>,
        f: impl FnMut() -> Result<T, Error>,
    ) -> Result<T, Error> {
        with_room(self.cache.upgrade().as_deref(), Some(self), f)
    }

    /// The handle the LRU keeps, opening the dependency again from its
    /// source when it was evicted (on [`Meter::reopen`] of its meter). The
    /// source is read and parsed with no lock held; the first open to
    /// publish wins, and publishes its handle and its meter in one step.
    /// A meter it replaces is dropped after the lock.
    pub(crate) fn handle(self: &Arc<Self>) -> Result<File, Error> {
        let (alive, meter) = {
            let open = lock(&self.open);
            (open.alive(), open.meter.clone())
        };
        if let Some(f) = alive {
            if let Some(cache) = self.cache.upgrade() {
                cache.touch(self, &f)?;
            }
            return Ok(f);
        }
        let meter = meter.reopen();
        let file = open_source(&self.name, &self.source, &meter, &self.ctx)?;
        let (file, _replaced) = {
            let mut open = lock(&self.open);
            if let Some(theirs) = open.alive() {
                (theirs, None)
            } else {
                let mine = Open::of(&file, meter);
                (file, Some(std::mem::replace(&mut *open, mine)))
            }
        };
        if let Some(cache) = self.cache.upgrade() {
            cache.touch(self, &file)?;
        }
        Ok(file)
    }

    /// The open handle if it is alive, without opening anything.
    pub(crate) fn alive(&self) -> Option<File> {
        lock(&self.open).alive()
    }

    /// The meter of its current open.
    pub(crate) fn meter(&self) -> Arc<Meter> {
        lock(&self.open).meter.clone()
    }
}
