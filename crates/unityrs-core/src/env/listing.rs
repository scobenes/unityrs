// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! UnityPy's `env.files` dict ([`Files`]) and the listings that walk it:
//! [`Environment::files`] and [`Environment::objects`]. "Module docs"
//! below are [`super`]'s.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::Arc;

use super::Environment;
use super::held::{self, Held};
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
    /// True for a `find_file` load with `is_dependency`
    /// ([`Environment::objects`] and [`Environment::container`] skip it),
    /// false for every other, and for a raw reader, to which UnityPy's
    /// `parse_file` gives no `is_dependency` (`getattr(f, "is_dependency",
    /// False)` reads it false).
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

/// One `files` entry, with the charge for its share of the list and the
/// indexes.
struct Listed {
    entry: FileEntry,
    _charge: Charge,
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

/// What one listed load costs the `files` tables: three elements of the
/// list and [`HASH_TABLE_SHARES`] buckets of each index (as a registry
/// slot's share, [`registry`]).
///
/// [`registry`]: super::registry
const LISTED_SHARE: u64 = 3 * size_of::<Listed>() as u64
    + 2 * HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<usize>() + 1) as u64;

impl Environment {
    fn files_read(&self) -> Held<std::sync::RwLockReadGuard<'_, Files>> {
        held::read(&self.inner.files)
    }

    fn files_write(&self) -> Held<std::sync::RwLockWriteGuard<'_, Files>> {
        held::write(&self.inner.files)
    }

    fn file_at(&self, i: usize) -> Option<FileEntry> {
        self.files_read().list.get(i).map(|l| l.entry.clone())
    }

    /// Adds `file` to `files` under `name` with dict semantics, charged to
    /// `meter`, the loaded file's: a new key's share of the list and the
    /// indexes, and its lowered copy for rule 4's lookup (a key copied from
    /// a resolver's name carries its own charge). Every load is charged so,
    /// a caller's included (review round 2: a caller's load charged
    /// nothing, and its lowered key was built on an unlimited meter). The
    /// lowered key's change is noted for the running container preload
    /// walks after it is published, whether or not one ran before
    /// (`super::watch`, invariant W1: review round 5 decided before the
    /// publication, so a walk that started in between kept a stale miss).
    pub(super) fn list(
        &self,
        name: RawStr,
        file: File,
        is_dependency: bool,
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let charge = meter.charge(LISTED_SHARE)?;
        // The lowered key: a new key's index entry, and in every case the
        // key a walk's lookup watches (`super::watch`).
        let lowered = py_lower(&name, meter)?;
        let watches = self.inner.registry.watches();
        let key_hash = watches.hash(lowered.as_bytes());
        #[cfg(test)]
        self.inner.registry.hook(super::registry::Hook::Listing);
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
                files.lowered.entry(lowered).or_insert(at);
                None
            }
        };
        watches.notify_hash(key_hash);
        drop(replaced);
        Ok(())
    }

    /// `files`, in insertion order: one entry per caller load, and with
    /// [`EnvOptions::keep_resolver_loads`] one per file the resolver loaded
    /// (module docs), a repeated key replacing the value in place. Each
    /// entry is charged to its file's meter (its share of the list and the
    /// indexes, its lowered key, and a resolver load's key copy, as a
    /// registry slot's are, [`registry`]). An iterator, as the other
    /// listings are (module docs): it takes the lock briefly per entry and
    /// allocates nothing but the entry it yields.
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
