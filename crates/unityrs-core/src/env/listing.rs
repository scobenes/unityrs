//! UnityPy's `env.files` dict ([`Files`]) and the listings that walk it:
//! [`Environment::files`] and [`Environment::objects`]. "Module docs"
//! below are [`super`]'s.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::{Arc, PoisonError};

use super::Environment;
use crate::error::Error;
use crate::files::{BundleFile, File, SerializedFile};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter};
use crate::object::ObjectRef;
use crate::pystr::py_lower;
use crate::raw::RawStr;

/// The iterator [`Environment::objects`] returns: a handle per nesting
/// level (the file list position, the bundle, the entry, the object index).
pub struct Objects<'a> {
    env: &'a Environment,
    next_file: usize,
    bundles: Vec<(Arc<BundleFile>, usize)>,
    current: Option<(Arc<SerializedFile>, usize)>,
}

const _: () = crate::send_sync::<Objects<'static>>();

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

const _: () = crate::send_sync::<FileEntry>();

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
pub(super) struct Files {
    list: Vec<Listed>,
    index: HashMap<RawStr, usize>,
    lowered: HashMap<RawStr, usize>,
}

/// What one listed resolver load costs the `files` tables: three elements
/// of the list and [`HASH_TABLE_SHARES`] buckets of each index (as a
/// registry slot's share, [`registry`]).
///
/// [`registry`]: super::registry
const LISTED_SHARE: u64 = 3 * size_of::<Listed>() as u64
    + 2 * HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<usize>() + 1) as u64;

impl Environment {
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

    /// Adds `file` to `files` under `name` with dict semantics. A resolver
    /// load's new key is charged to `meter` (its key's copy rides in the
    /// key; this charges its share of the list and the index).
    pub(super) fn list(
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

    /// `files`, in insertion order: one entry per caller load, and with
    /// [`EnvOptions::keep_resolver_loads`] one per file the resolver loaded
    /// (module docs), a repeated key replacing the value in place. A
    /// resolver load's entry is charged to that file's meter (its key and its
    /// share of the list, as a registry slot's are, [`registry`]), since the
    /// input chooses how many there are. An iterator, as the other listings
    /// are (module docs): it takes the lock briefly per entry and allocates
    /// nothing but the entry it yields.
    ///
    /// [`EnvOptions::keep_resolver_loads`]: super::EnvOptions::keep_resolver_loads
    /// [`registry`]: super::registry
    #[must_use]
    pub fn files(&self) -> FileEntries<'_> {
        FileEntries { env: self, next: 0 }
    }

    /// The first `files` entry whose key lower-cases to `clean` (the
    /// Environment as a top-level file's container, rule 4), by the lowered
    /// index ([`Files`]).
    pub(super) fn files_match(&self, clean: &RawStr) -> Option<File> {
        let files = self.files_read();
        let at = *files.lowered.get(clean.as_bytes())?;
        files.list.get(at).map(|l| l.entry.file.clone())
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
}
