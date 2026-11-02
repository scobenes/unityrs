//! [`Environment::resource`]: UnityPy's `get_resource_data`, over the
//! candidates of [`super::resource`]. "Module docs" below are [`super`]'s.

use std::sync::Arc;

use bytes::Bytes;

use super::registry::{Slot, Through, entry_of};
use super::{Cab, Candidate, Environment, LoadMode, RawCab, resource_candidates};
use crate::error::{Error, NotFound};
use crate::files::{BundleFile, File, Parent, ParentKind, SerializedFile};
use crate::raw::RawStr;

/// `ResourceReader._get_resource_data(reader, offset, size)`
/// (`helpers/ResourceReader.py:36-38`): `reader.Position = offset;
/// reader.read_bytes(size)`, by the reader's rules: a bundle entry's and a
/// file of bytes' are a Python slice of the reader's bytes; a file loaded
/// from a path is a stream, which takes the offset and reads the size by
/// the stream reader's rules ([`crate::stream::ReaderKind::seek`] and
/// [`crate::stream::ReaderKind::read_u64`]; `PARITY.md` section 13, and
/// section 19.3's row "a path's stream past the machine's limits" for what
/// it refuses where UnityPy's outcome depends on the machine). Review round
/// 10: this restated a bound for the size that was not the reader's.
fn read_cab(cab: &Cab, offset: u64, size: u64) -> Result<Bytes, Error> {
    match cab {
        Cab::Raw(RawCab::InBundle { bundle, dir_index }) => {
            bundle.entry_range(*dir_index, offset..offset.saturating_add(size))
        }
        Cab::Raw(RawCab::File(raw)) => {
            let kind = raw.reader_kind();
            let at = kind.seek(i128::from(offset))?;
            let (range, _) = kind.read_u64(raw.data.len(), at, size)?;
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

impl Environment {
    /// `get_resource_data(path, from, offset, size)`
    /// (`helpers/ResourceReader.py:10-38`). The candidate is resolved by
    /// UnityPy's rule (the registry, then dependencies), but each candidate's
    /// slot is first *located* without being parsed. When the core can tell,
    /// without reading anything, that the slot is an entry of `from`'s own
    /// input, the entry is read through `from`'s bundle and the shared slot
    /// is never parsed, so neither the resolution nor the read decodes
    /// anything through the shared handle or charges its pool (plan section
    /// 3.13): the slot's open handle is `from`'s bundle itself, or the slot
    /// is a [`Source::Bytes`](super::Source::Bytes) whose `Bytes` are `from`'s bundle's source (the
    /// same allocation and range) at the entry's directory index. A slot whose
    /// source is a [`Source::Path`](super::Source::Path) or
    /// [`Source::Fetch`](super::Source::Fetch) and whose handle is
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
    ///
    /// A pool refusal (a read or a lookup from a dependency's file) evicts
    /// the others and retries once, `from`'s top-level bundle held
    /// meanwhile: each lookup of the candidates, and each candidate's name
    /// with its fallback load, the load taking its own retry, so a retry
    /// never loads again what a load before it loaded (plan section 3.13;
    /// review round 8: the read had no retry).
    ///
    /// [`EnvOptions::keep_resolver_loads`]: super::EnvOptions::keep_resolver_loads
    /// [`FileEntry`]: super::FileEntry
    /// [`resource::resource_candidates`]: super::resource::resource_candidates
    pub fn resource(
        &self,
        from: &Arc<SerializedFile>,
        path: &RawStr,
        offset: u64,
        size: u64,
    ) -> Result<Bytes, Error> {
        let _top = top_bundle(from);
        // UnityPy helpers/ResourceReader.py:11-33
        let candidates = resource_candidates(path);
        let first = self.room(|| {
            self.reach_chain(from)?;
            self.resource_try(from, &candidates, offset, size)
        })?;
        if let Some(bytes) = first {
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
            self.room(|| self.fallback_load(&c.joined(meter)?))?;
        }
        if let Some(bytes) = self.room(|| self.resource_try(from, &candidates, offset, size))? {
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
                    File::Bundle(nested) => {
                        self.inner.registry.reach(&slot, Some(&nested))?;
                        continue;
                    }
                    File::Web(_) => continue,
                }
            }
            if let Some(cab) = self.inner.registry.resolve(&slot)? {
                return Ok(Some(read_cab(&cab, offset, size)?));
            }
        }
        Ok(None)
    }
}

impl Environment {
    /// Under [`LoadMode::Lazy`], reaches the nested container on `from`'s
    /// own chain before a lookup from it answers (the registry's module
    /// docs): the container below `from`'s top-level bundle, whose reach
    /// registers its whole subtree, when the slot under its name is that
    /// very entry, or stands in for it ([`reach_entry`]). When the slot is
    /// another entry's, `from`'s container was dropped by a later
    /// registration, which reached it then. Under `Eager` a nested container
    /// is registered at load. Walks up `from`'s parents only (as deep as
    /// [`crate::limits::Limits::max_container_depth`]), allocating nothing.
    pub(super) fn reach_chain(&self, from: &SerializedFile) -> Result<(), Error> {
        if self.inner.options.load != LoadMode::Lazy {
            return Ok(());
        }
        let Some(mut top) = parent_bundle(from.parent()) else {
            return Ok(());
        };
        let mut below = None;
        while let Some(up) = parent_bundle(top.parent()) {
            below = Some(std::mem::replace(&mut top, up));
        }
        let Some(nested) = below else {
            return Ok(());
        };
        let Some(slot) = self.inner.registry.slot(nested.name(), nested.meter())? else {
            return Ok(());
        };
        self.reach_entry(&top, &slot, &nested)
    }

    /// A pointer reached `nested`, `from`'s sibling at `dir_index` of its
    /// bundle `bundle`, by the bundle's names (`classes/PPtr.py:75-78`):
    /// under [`LoadMode::Lazy`] it is reached as a `get` of its name would
    /// reach it, when the slot under its name is that entry, or stands in
    /// for it ([`reach_entry`]).
    pub(super) fn reach_sibling(
        &self,
        from: &SerializedFile,
        bundle: &Arc<BundleFile>,
        dir_index: usize,
        nested: &Arc<BundleFile>,
    ) -> Result<(), Error> {
        if self.inner.options.load != LoadMode::Lazy {
            return Ok(());
        }
        let Some(d) = bundle.directory().get(dir_index) else {
            return Ok(());
        };
        let Some(slot) = self.inner.registry.slot(&d.path, from.meter())? else {
            return Ok(());
        };
        self.reach_entry(bundle, &slot, nested)
    }

    /// Reaches `slot` when it is `nested`, the asking file's handle on a
    /// nested container of its bundle `bundle`, as [`entry_of`] tells it:
    /// from `nested` when the slot reads through `bundle` itself
    /// ([`Through::Own`]), through the slot's own handle when it stands in
    /// for that entry ([`Through::Slot`], a reload's slot: review round 5
    /// found the earlier load's files reached nothing).
    fn reach_entry(
        &self,
        bundle: &Arc<BundleFile>,
        slot: &Arc<Slot>,
        nested: &Arc<BundleFile>,
    ) -> Result<(), Error> {
        let Some((i, through)) = entry_of(bundle, slot) else {
            return Ok(());
        };
        if !matches!(bundle.entry(i), Ok(File::Bundle(b)) if Arc::ptr_eq(&b, nested)) {
            return Ok(());
        }
        let reg = &self.inner.registry;
        match through {
            Through::Own => reg.reach(slot, Some(nested)),
            Through::Slot => reg.reach(slot, None),
        }
    }
}

/// The top-level bundle `from` is read through, while it is alive: its
/// container's container, and so on up. A call that reads from `from` holds
/// it, so an eviction while it runs (its own retry's, another thread's)
/// drops only the LRU's handle, and the chain the call reads through stays.
pub(super) fn top_bundle(from: &SerializedFile) -> Option<Arc<BundleFile>> {
    let mut top = parent_bundle(from.parent())?;
    while let Some(up) = parent_bundle(top.parent()) {
        top = up;
    }
    Some(top)
}

/// The bundle a parent link names, while it is alive.
fn parent_bundle(parent: Option<&Parent>) -> Option<Arc<BundleFile>> {
    match &parent?.kind {
        ParentKind::Bundle { bundle, .. } => bundle.upgrade(),
        ParentKind::Web => None,
    }
}

/// When `slot` is an entry of `from`'s own input read through `from`'s own
/// bundle ([`Through::Own`]: the core can tell without reading anything,
/// plan section 3.13), that bundle and the entry's directory index.
fn own_entry(from: &SerializedFile, slot: &Slot) -> Option<(Arc<BundleFile>, usize)> {
    let fb = parent_bundle(from.parent())?;
    match entry_of(&fb, slot)? {
        (i, Through::Own) => Some((fb, i)),
        (_, Through::Slot) => None,
    }
}
