//! [`Environment::resource`]: UnityPy's `get_resource_data`, over the
//! candidates of [`super::resource`]. "Module docs" below are [`super`]'s.

use std::sync::Arc;

use bytes::Bytes;

use super::registry::{Slot, SlotKind};
use super::{Cab, Candidate, Environment, LoadMode, RawCab, Source, resource_candidates};
use crate::error::{Error, NotFound};
use crate::files::{BundleFile, File, ParentKind, SerializedFile};
use crate::raw::RawStr;
use crate::stream::py_read;

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
                    File::Bundle(nested) => {
                        self.expand_own(&slot, &nested)?;
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
    /// What a `get` of a nested container's name does, for a container
    /// reached through the asking file's own bundle instead (a resource
    /// candidate, read so; a pointer to the file's sibling): under
    /// [`LoadMode::Lazy`] the container's name is a key, and when `slot`,
    /// the slot under it, is that very entry ([`own_entry`]), its entries
    /// are registered as resolving the slot registers them (`PARITY.md`
    /// section 19.3, difference (b)), from `nested`, the reader's handle on
    /// the container, so nothing is reached through the slot's own handle
    /// (plan section 3.13). Under `Eager` a nested container has no key,
    /// its entries registered at load, and no slot is one.
    ///
    /// [`LoadMode::Lazy`]: super::LoadMode::Lazy
    pub(super) fn expand_own(
        &self,
        slot: &Arc<Slot>,
        nested: &Arc<BundleFile>,
    ) -> Result<(), Error> {
        let dep = match &slot.kind {
            SlotKind::Entry { .. } => None,
            SlotKind::Dependency { dep } => Some(&**dep),
            SlotKind::Ready(_) => return Ok(()),
        };
        self.inner.registry.expand(slot, nested, dep)
    }

    /// [`Self::expand_own`] for `from`'s sibling at `dir_index` of its
    /// bundle `bundle`, which a pointer reached by the bundle's names
    /// (`classes/PPtr.py:75-78`): when the slot under its name is that
    /// entry.
    pub(super) fn expand_sibling(
        &self,
        from: &SerializedFile,
        bundle: &BundleFile,
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
        if own_entry(from, &slot).is_some_and(|(_, i)| i == dir_index) {
            self.expand_own(&slot, nested)?;
        }
        Ok(())
    }
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
