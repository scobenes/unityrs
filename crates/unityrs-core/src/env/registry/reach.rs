//! Reaching a lazy slot (module docs, "Nested containers under
//! `LoadMode::Lazy`"): [`Registry::reach`], the one routine that registers
//! a nested container's entries, and [`Registry::settle`], which reaches
//! the lazy slots registrations dropped.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::Source;
use super::{By, DepSlot, Dependency, Registry, Slot, SlotKind, lock};
use crate::detect::{FileType, check_file_type};
use crate::error::Error;
use crate::files::{BundleFile, File};
use crate::limits::Limits;
use crate::once::CellError;

/// The bytes of an entry read to tell a bundle. `check_file_type` calls the
/// entry one by the NUL-terminated string in its first 20 bytes, which it
/// reads alike from these 21 and from the whole entry (`py_string_to_null`
/// stops at the 20th byte without a NUL only when a 21st follows), and a
/// prefix this short never reaches the gzip test, the only one that reads
/// the `Limits` it is given.
const PEEK: u64 = 21;

thread_local! {
    /// Whether this thread is in [`Registry::settle`]'s loop: a slot a
    /// reach's own registrations drop is left to that loop, so a chain of
    /// drops is reached one after another, never nested on the stack.
    static SETTLING: Cell<bool> = const { Cell::new(false) };
}

/// Clears [`SETTLING`] when the loop that set it ends, by return or panic.
struct Settling;

impl Drop for Settling {
    fn drop(&mut self) {
        SETTLING.with(|s| s.set(false));
    }
}

/// Whether entry `i` of `b` is a bundle, from its first [`PEEK`] bytes
/// (their blocks decoded, as any read decodes them). A read that fails other
/// than transiently says no: such an entry cannot be parsed, so it holds
/// nothing to register, and its reads fail with the error.
fn is_bundle_entry(b: &BundleFile, i: usize) -> Result<bool, Error> {
    match b.entry_range(i, 0..PEEK) {
        Ok(head) => Ok(matches!(
            check_file_type(&head, &Limits::default()),
            Ok(FileType::BundleFile)
        )),
        Err(e) if e.is_transient() => Err(e),
        Err(_) => Ok(false),
    }
}

/// Whether two slots are one entry of one input, registered by two loads of
/// it: the later load's slot stands for the earlier's, so dropping the
/// earlier one unreached loses nothing an eager load keeps but the
/// positions of its entries' keys (module docs). The same source is the
/// same `Bytes` (allocation and length), the same path, or the same
/// fetcher, as a dependency's reopen takes it to be.
pub(super) fn same_entry(one: &Slot, other: &Slot) -> bool {
    if one.rank.path() != other.rank.path() {
        return false;
    }
    match (&one.kind, &other.kind) {
        (
            SlotKind::Entry { bundle, dir_index },
            SlotKind::Entry {
                bundle: bundle2,
                dir_index: dir_index2,
            },
        ) => {
            dir_index == dir_index2
                && (Arc::ptr_eq(bundle, bundle2)
                    || (bundle.parent().is_none()
                        && bundle2.parent().is_none()
                        && same_bytes(bundle.source(), bundle2.source())))
        }
        (SlotKind::Dependency { dep }, SlotKind::Dependency { dep: dep2 }) => {
            same_source(&dep.source, &dep2.source)
        }
        _ => false,
    }
}

/// One allocation and length.
fn same_bytes(data: &[u8], data2: &[u8]) -> bool {
    data.as_ptr() == data2.as_ptr() && data.len() == data2.len()
}

/// The same `Bytes`, path or fetcher.
fn same_source(source: &Source, source2: &Source) -> bool {
    match (source, source2) {
        (Source::Bytes { data, .. }, Source::Bytes { data: data2, .. }) => same_bytes(data, data2),
        (Source::Path(path), Source::Path(path2)) => path == path2,
        (Source::Fetch { fetch, .. }, Source::Fetch { fetch: fetch2, .. }) => {
            Arc::ptr_eq(fetch, fetch2)
        }
        _ => false,
    }
}

impl Registry {
    /// Reaches `slot` (module docs): for a lazy slot not yet reached, reads
    /// the entry's first bytes and, when they are a bundle's, parses it and
    /// registers its whole subtree at the ranks below the slot's, as the
    /// slot's own kind registers (entries of the parsed bundle for a
    /// caller's, dependency slots for a dependency's), a container inside it
    /// with no key of its own, as an eager load registers it; then marks the
    /// slot reached. `nested` is the container when the caller holds it: the
    /// slot's own parse, or the asking file's handle on the very entry the
    /// slot is (`super::super::resource_data`'s `own_entry`), so nothing is
    /// read through the slot's handle (plan section 3.13). Every caller that
    /// finds the slot unreached registers the subtree, and a second
    /// registration of an entry changes nothing, so each returns with every
    /// entry registered. Only a transient failure (a refusal) is returned,
    /// and the slot stays unreached for the next caller.
    pub(crate) fn reach(
        &self,
        slot: &Arc<Slot>,
        nested: Option<&Arc<BundleFile>>,
    ) -> Result<(), Error> {
        self.reach_by(slot, nested, true)
    }

    /// [`Self::reach`]; with `reopen` false, a dependency the LRU evicted is
    /// not opened again to read the entry, and its slot counts as reached.
    fn reach_by(
        &self,
        slot: &Arc<Slot>,
        nested: Option<&Arc<BundleFile>>,
        reopen: bool,
    ) -> Result<(), Error> {
        if !slot.unreached() {
            return Ok(());
        }
        let parsed = if nested.is_none() {
            self.lazy_container(slot, reopen)?
        } else {
            None
        };
        if let Some(nested) = nested.or(parsed.as_ref()) {
            let dep = match &slot.kind {
                SlotKind::Dependency { dep } => Some(&**dep),
                SlotKind::Entry { .. } | SlotKind::Ready(_) => None,
            };
            let mut path = slot.rank.path().to_vec();
            self.register_tree(nested, dep, slot.rank.load, &mut path)?;
        }
        slot.mark_reached();
        Ok(())
    }

    /// The nested container a lazy slot is, if it is one: the entry's first
    /// bytes read through the slot's own handle (an evicted dependency's
    /// opened again only with `reopen`), and the entry parsed only when they
    /// are a bundle's.
    fn lazy_container(&self, slot: &Slot, reopen: bool) -> Result<Option<Arc<BundleFile>>, Error> {
        let (bundle, i) = match &slot.kind {
            SlotKind::Entry { bundle, dir_index } => (bundle.clone(), *dir_index),
            SlotKind::Dependency { dep } => {
                let &[i] = slot.rank.path() else {
                    return Ok(None);
                };
                let top = if reopen {
                    Some(dep.handle()?)
                } else {
                    dep.alive()
                };
                let Some(File::Bundle(b)) = top else {
                    return Ok(None);
                };
                (b, i)
            }
            SlotKind::Ready(_) => return Ok(None),
        };
        if !is_bundle_entry(&bundle, i)? {
            return Ok(None);
        }
        match bundle.entry(i) {
            Ok(File::Bundle(nested)) => {
                self.count(slot);
                Ok(Some(nested))
            }
            Err(e) if e.is_transient() => Err(e),
            Ok(_) | Err(_) => Ok(None),
        }
    }

    /// Registers every entry of `nested`, at `path` and below, as an eager
    /// load does: a bundle entry's own entries in its place, every other
    /// entry as a slot of the kind `dep` says, known to be no container. An
    /// entry that fails other than transiently is registered as it is, so
    /// its reads fail with its error (difference (a)).
    fn register_tree(
        &self,
        nested: &Arc<BundleFile>,
        dep: Option<&Arc<Dependency>>,
        load: u64,
        path: &mut Vec<usize>,
    ) -> Result<(), Error> {
        for (j, d) in nested.directory().iter().enumerate() {
            path.push(j);
            let r = self.register_tree_entry(nested, j, &d.path, dep, load, path);
            path.pop();
            r?;
        }
        Ok(())
    }

    fn register_tree_entry(
        &self,
        nested: &Arc<BundleFile>,
        j: usize,
        name: &crate::raw::RawStr,
        dep: Option<&Arc<Dependency>>,
        load: u64,
        path: &mut Vec<usize>,
    ) -> Result<(), Error> {
        if is_bundle_entry(nested, j)? {
            match nested.entry(j) {
                Ok(File::Bundle(inner)) => {
                    self.parsed.fetch_add(1, Ordering::Relaxed);
                    return self.register_tree(&inner, dep, load, path);
                }
                Err(e) if e.is_transient() => return Err(e),
                Ok(_) | Err(_) => {}
            }
        }
        let kind = match dep {
            Some(dep) => SlotKind::Dependency {
                dep: DepSlot::new(dep),
            },
            None => SlotKind::Entry {
                bundle: nested.clone(),
                dir_index: j,
            },
        };
        self.insert(name, kind, load, path, nested.meter(), By::Reach, false)
    }

    /// Adds a dropped lazy slot to the list [`Self::settle`] reaches.
    pub(super) fn defer(&self, slot: Arc<Slot>) {
        let mut pending = lock(&self.pending);
        pending.push(slot);
        self.pending_len.store(pending.len(), Ordering::Release);
    }

    /// Takes a slot off the list, which keeps at most four elements' room
    /// per element it holds (and four more): what [`super::PENDING_SHARE`]
    /// pays for.
    fn next_deferred(&self) -> Option<Arc<Slot>> {
        let mut pending = lock(&self.pending);
        let slot = pending.pop();
        if pending.capacity() > 4 * pending.len() + 4 {
            let keep = 2 * pending.len();
            pending.shrink_to(keep);
        }
        self.pending_len.store(pending.len(), Ordering::Release);
        slot
    }

    /// Reaches every lazy slot a registration dropped unreached (module
    /// docs), one after another: the registrations a reach makes may drop
    /// more, which this loop takes too. A dropped slot is no key's value, so
    /// nothing could reach it by name afterwards; an eager load registered
    /// its entries at load. A dependency the LRU has evicted is not opened
    /// again for it (its slot counts as reached: module docs, what remains
    /// of (b)). A refusal leaves the slot on the list for the next call
    /// (every registration, every lookup by name and every container pass
    /// calls this) and is returned; a registration or a lookup by name goes
    /// on without it, as without any container not yet reached, and a
    /// container pass fails with it. Any other failure means the dropped
    /// entry cannot be parsed, so there is nothing to register.
    pub(crate) fn settle(&self) -> Result<(), Error> {
        if self.pending_len.load(Ordering::Acquire) == 0 || SETTLING.with(Cell::get) {
            return Ok(());
        }
        SETTLING.with(|s| s.set(true));
        let _settling = Settling;
        while let Some(slot) = self.next_deferred() {
            if let Err(e) = self.reach_by(&slot, None, false) {
                if e.is_transient() {
                    self.defer(slot);
                    return Err(e);
                }
                slot.mark_reached();
            }
        }
        Ok(())
    }
}
