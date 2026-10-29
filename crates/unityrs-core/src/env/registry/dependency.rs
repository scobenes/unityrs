//! A dependency's record ([`Dependency`]): what reopens it after an
//! eviction, and the weak handle on the file the LRU keeps open; and
//! [`open_source`], which reads a source, detects it and parses it.

use std::sync::{Arc, Mutex, Weak};

use super::{DepCache, lock};
use crate::detect::check_file_type;
use crate::env::Source;
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, RawFile, SerializedFile, parse_file};
use crate::limits::Meter;
use crate::options::FileContext;
use crate::pystr::is_split_path;
use crate::raw::RawStr;

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
