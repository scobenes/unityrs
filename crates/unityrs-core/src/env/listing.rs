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
use super::watch::{Table, Under};
use crate::error::Error;
use crate::files::{BundleFile, File, SerializedFile};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter};
use crate::object::ObjectRef;
use crate::pystr::py_lower;
use crate::raw::RawStr;

/// The iterator [`Environment::objects`] returns: a handle per nesting
/// level (the position of the last `files` entry it stepped into, the
/// bundle, the entry, the object index).
pub struct Objects<'a> {
    env: &'a Environment,
    after: Option<u64>,
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
            let (at, entry) = self.env.file_after(self.after)?;
            self.after = Some(at);
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

/// The iterator [`Environment::files`] returns: a cursor at the position
/// of the last entry it yielded, which entries listed meanwhile do not
/// disturb.
#[derive(Debug)]
pub struct FileEntries<'a> {
    env: &'a Environment,
    after: Option<u64>,
}

const _: () = crate::send_sync::<FileEntries<'static>>();

impl Iterator for FileEntries<'_> {
    type Item = FileEntry;

    fn next(&mut self) -> Option<FileEntry> {
        let (at, entry) = self.env.file_after(self.after)?;
        self.after = Some(at);
        Some(entry)
    }
}

/// One `files` entry, with the loads of its rank and the charge for its
/// share of the list and the indexes.
struct Listed {
    entry: FileEntry,
    /// The key's position: the least load that listed it.
    position: u64,
    /// The load whose file is the value: the greatest that listed the key.
    value: u64,
    _charge: Charge,
}

/// `env.files`: a dict by rank, as the registry is ([`super::registry`]'s
/// module docs, "Ranks"): each key at the least number of the loads that
/// listed it, with the file of the greatest, whatever the order the loads
/// list in (review round 16: the list took the order the threads listed
/// in while the registry took the loads', so a load that took the lower
/// number but listed last won `files` and lost the registry, for good).
/// A list by position, an index of each key's position, and each key
/// lower-cased to the least position of a key that lowers to it (the scan
/// of `files` a top-level file's pointer makes, rule 4, as one lookup). A
/// load's number is the position of at most the one key it lists, so
/// positions are distinct.
#[derive(Default)]
pub(super) struct Files {
    list: Vec<Listed>,
    index: HashMap<RawStr, u64>,
    lowered: HashMap<RawStr, u64>,
    /// The running container preload walks' flags under lowered keys, set
    /// in the step that publishes a key's change (`super::watch`).
    watched: Table,
}

impl Files {
    /// The entry at `position`.
    fn at(&self, position: u64) -> Option<&Listed> {
        let i = self
            .list
            .binary_search_by_key(&position, |l| l.position)
            .ok()?;
        self.list.get(i)
    }

    /// Key `name`, at `position`, listed again by load `load`
    /// ([`Environment::list`]): it takes `file` (and `is_dependency`) only
    /// if `load` is greater than its value's load, and moves to `load`, and
    /// so does the least position of its lowered key `lowered`, if that is
    /// less than its position. Returns the file it does not keep.
    fn relist(
        &mut self,
        name: &RawStr,
        position: u64,
        file: File,
        is_dependency: bool,
        load: u64,
        lowered: &RawStr,
    ) -> File {
        let Ok(i) = self.list.binary_search_by_key(&position, |l| l.position) else {
            // Never: every key of the index is listed at its position.
            return file;
        };
        let Some(entry) = self.list.get_mut(i) else {
            return file;
        };
        let unused = if load > entry.value {
            entry.value = load;
            entry.entry.is_dependency = is_dependency;
            std::mem::replace(&mut entry.entry.file, file)
        } else {
            file
        };
        if load < position {
            let mut moved = self.list.remove(i);
            moved.position = load;
            let at = self.list.partition_point(|l| l.position < load);
            self.list.insert(at, moved);
            if let Some(p) = self.index.get_mut(name.as_bytes()) {
                *p = load;
            }
            if let Some(least) = self.lowered.get_mut(lowered.as_bytes()) {
                *least = (*least).min(load);
            }
        }
        unused
    }
}

impl Under for std::sync::RwLock<Files> {
    /// Under the listing's write lock, which publishes every entry
    /// ([`Environment::list`]).
    fn with_table(&self, f: &mut dyn FnMut(&mut Table)) {
        f(&mut held::write(self).watched);
    }
}

/// What one listed load costs the `files` tables: three elements of the
/// list and [`HASH_TABLE_SHARES`] buckets of each index (as a registry
/// slot's share, [`registry`]).
///
/// [`registry`]: super::registry
const LISTED_SHARE: u64 = 3 * size_of::<Listed>() as u64
    + 2 * HASH_TABLE_SHARES * (size_of::<RawStr>() + size_of::<u64>() + 1) as u64;

/// A listing charged and not yet published ([`Environment::charge_listing`]):
/// its key, the key lowered, and its share of the tables.
pub(super) struct Listing {
    name: RawStr,
    lowered: RawStr,
    charge: Charge,
}

impl Environment {
    fn files_read(&self) -> Held<std::sync::RwLockReadGuard<'_, Files>> {
        held::read(&self.inner.files)
    }

    fn files_write(&self) -> Held<std::sync::RwLockWriteGuard<'_, Files>> {
        held::write(&self.inner.files)
    }

    /// The entries of both tables of the walks' flags (the registry's and
    /// the listing's), for a test.
    #[cfg(test)]
    pub(super) fn watch_entries(&self) -> usize {
        self.inner.registry.watch_room().0 + self.files_read().watched.room().0
    }

    /// The first entry after position `after` (the first of all for
    /// `None`), and its position.
    fn file_after(&self, after: Option<u64>) -> Option<(u64, FileEntry)> {
        let files = self.files_read();
        let i = after.map_or(0, |a| files.list.partition_point(|l| l.position <= a));
        files.list.get(i).map(|l| (l.position, l.entry.clone()))
    }

    /// Adds `file` to `files` under `name` with dict semantics by rank, as
    /// the listing of load `load` ([`Files`]): a new key takes its place at
    /// `load`; a known key takes `file` only if `load` is greater than its
    /// value's load, and moves to `load` if that is less than its
    /// position; the file it does not keep is dropped after the lock.
    /// Charged to `meter`, the loaded file's: a new key's share of the list and the
    /// indexes, and its lowered copy for rule 4's lookup (a key copied from
    /// a resolver's name carries its own charge). Every load is charged so,
    /// a caller's included (review round 2: a caller's load charged
    /// nothing, and its lowered key was built on an unlimited meter). The
    /// lowered key's change sets the running container preload walks'
    /// flags under it in the step that publishes it, under the write lock
    /// (`super::watch`, invariant W1: review round 5 decided before the
    /// publication whether a walk ran, so a walk that started in between
    /// kept a stale miss; review round 14 set the flags after the lock, so
    /// a walk could find the entry while its kept miss of it still held).
    pub(super) fn list(
        &self,
        name: RawStr,
        file: File,
        is_dependency: bool,
        load: u64,
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        let listing = Self::charge_listing(name, meter)?;
        self.publish_listing(listing, file, is_dependency, load);
        Ok(())
    }

    /// The charged half of [`Self::list`]: every charge a listing makes,
    /// to `meter`, before anything is published (a dependency's load under
    /// `Lazy` charges its listing before it publishes its keys, the
    /// registry's module docs, "A failed dependency load").
    pub(super) fn charge_listing(name: RawStr, meter: &Arc<Meter>) -> Result<Listing, Error> {
        let charge = meter.charge(LISTED_SHARE)?;
        // The lowered key: a new key's index entry, and in every case the
        // key a walk's lookup watches (`super::watch`).
        let lowered = py_lower(&name, meter)?;
        Ok(Listing {
            name,
            lowered,
            charge,
        })
    }

    /// The published half of [`Self::list`], which nothing can refuse.
    pub(super) fn publish_listing(
        &self,
        listing: Listing,
        file: File,
        is_dependency: bool,
        load: u64,
    ) {
        let Listing {
            name,
            lowered,
            charge,
        } = listing;
        let key_hash = self.inner.registry.watches().hash(lowered.as_bytes());
        #[cfg(test)]
        self.inner.registry.hook(super::registry::Hook::Listing);
        let unused = {
            let mut files = self.files_write();
            let unused = if let Some(&position) = files.index.get(name.as_bytes()) {
                Some(files.relist(&name, position, file, is_dependency, load, &lowered))
            } else {
                let at = files.list.partition_point(|l| l.position < load);
                files.list.insert(
                    at,
                    Listed {
                        entry: FileEntry {
                            name: name.clone(),
                            file,
                            is_dependency,
                        },
                        position: load,
                        value: load,
                        _charge: charge,
                    },
                );
                files.index.insert(name, load);
                let least = files.lowered.entry(lowered).or_insert(load);
                *least = (*least).min(load);
                None
            };
            // The entry and the setting of the walks' flags under its
            // lowered key are one step (`super::watch`, W1).
            files.watched.notify(key_hash);
            unused
        };
        #[cfg(test)]
        self.inner.registry.hook(super::registry::Hook::Listed);
        drop(unused);
    }

    /// `files`, in the order of the keys' positions (the least load that
    /// listed each, `listing.rs`'s `Files`), as UnityPy's dict iterates when the loads
    /// are made one after another in the order of their numbers: one entry per caller load, and with
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
        FileEntries {
            env: self,
            after: None,
        }
    }

    /// The first `files` entry whose key lower-cases to `clean` (the
    /// Environment as a top-level file's container, rule 4), by the lowered
    /// index ([`Files`]).
    pub(super) fn files_match(&self, clean: &RawStr) -> Option<File> {
        let files = self.files_read();
        let at = *files.lowered.get(clean.as_bytes())?;
        files.at(at).map(|l| l.entry.file.clone())
    }

    /// `objects`: every object of every non-dependency file of
    /// [`Self::files`], in its order, as an iterator (module docs: no
    /// allocation per object). A file that fails to load or parse on the way
    /// yields its error.
    #[must_use]
    pub fn objects(&self) -> Objects<'_> {
        Objects {
            env: self,
            after: None,
            bundles: Vec::new(),
            current: None,
        }
    }
}
