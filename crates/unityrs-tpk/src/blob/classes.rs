// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `ClassInformation` (`helpers/Tpk.py:236-237`): every class's
//! `TpkClassInformation` (`:333-345`), each entry a version and a
//! `TpkUnityClass` (`:289-305`) or `None`.

use std::cmp::Reverse;
use std::sync::Arc;

use unityrs_core::UnityVersion;
use unityrs_core::limits::{Meter, MeteredVec};

use super::{Stream, bounded, memory, range_count, read_version};
use crate::TpkError;

/// `TpkUnityClassFlags.HasEditorRootNode` (`Tpk.py:134`).
const HAS_EDITOR_ROOT_NODE: u8 = 64;
/// `TpkUnityClassFlags.HasReleaseRootNode` (`Tpk.py:135`).
const HAS_RELEASE_ROOT_NODE: u8 = 128;

/// A `TpkUnityClass` as the tree walk reads it: its release root, the one
/// field `generate_node` uses (`Tpk.py:64`, `:68`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnityClass {
    pub(crate) release_root: Option<u16>,
}

impl UnityClass {
    /// `TpkUnityClass(stream)` (`Tpk.py:298-305`): `<HHb` (the name, the
    /// base, the flags), then each root under its own flag, the editor root
    /// first. The flags are a signed byte, which `TpkUnityClassFlags` takes as
    /// the same eight bits, so every byte is a valid set.
    fn read(s: &mut Stream<'_>) -> Result<Self, TpkError> {
        s.u16("a class's name")?;
        s.u16("a class's base")?;
        let flags = s.u8("a class's flags")?;
        if flags & HAS_EDITOR_ROOT_NODE != 0 {
            s.u16("a class's editor root")?;
        }
        let release_root = if flags & HAS_RELEASE_ROOT_NODE == 0 {
            None
        } else {
            Some(s.u16("a class's release root")?)
        };
        Ok(Self { release_root })
    }
}

/// One entry of a `TpkClassInformation`: `(UnityVersion,
/// Optional[TpkUnityClass])`, `None` for the class removed at that version.
pub(crate) struct ClassEntry {
    pub(crate) version: UnityVersion,
    pub(crate) class: Option<UnityClass>,
}

/// A class of `ClassInformation`: its id, where it was listed, and its
/// entries' place in [`ClassInformation`]'s one table of entries.
struct Class {
    id: i32,
    listing: usize,
    first: usize,
    len: usize,
}

/// `ClassInformation`, `{x.ID: x for x in (TpkClassInformation(stream) for
/// _ in range(classCount))}` (`Tpk.py:236-237`): every class's entries in
/// their stored order, in one table, and the classes sorted by id with each
/// id's last listing kept, as the `dict` keeps it.
pub(crate) struct ClassInformation {
    classes: MeteredVec<Class>,
    entries: MeteredVec<ClassEntry>,
}

impl ClassInformation {
    /// `classCount` classes, each a `TpkClassInformation(stream)`
    /// (`Tpk.py:336-345`): its id, its entry count (a `range`: a negative
    /// one is none), then each entry's version and a class flag byte
    /// (`stream.read(1)[0]`), a class following a nonzero one. The entries
    /// of every class together are one table, bounded as one.
    pub(crate) fn read(s: &mut Stream<'_>, meter: &Arc<Meter>) -> Result<Self, TpkError> {
        let count = bounded("classes", range_count(s.i32("the class count")?))?;
        let mut classes = MeteredVec::new(meter);
        let mut entries = MeteredVec::new(meter);
        for listing in 0..count {
            let id = s.i32("a class's id")?;
            let n = range_count(s.i32("a class's entry count")?);
            let first = entries.len();
            let total = u64::try_from(first).unwrap_or(u64::MAX).saturating_add(n);
            let len = bounded("class entries", total)?.saturating_sub(first);
            for _ in 0..len {
                let version = read_version(s, "an entry's version")?;
                let class = if s.u8("an entry's class flag")? == 0 {
                    None
                } else {
                    Some(UnityClass::read(s)?)
                };
                entries
                    .push(ClassEntry { version, class })
                    .map_err(memory)?;
            }
            classes
                .push(Class {
                    id,
                    listing,
                    first,
                    len,
                })
                .map_err(memory)?;
        }
        // The dict: by id, each id's last listing first, then that one kept.
        let sorted = classes.as_mut_slice();
        sorted.sort_unstable_by_key(|c| (c.id, Reverse(c.listing)));
        let mut kept = 0_usize;
        for at in 0..sorted.len() {
            let id = sorted.get(at).map(|c| c.id);
            let last = kept
                .checked_sub(1)
                .and_then(|k| sorted.get(k))
                .map(|c| c.id);
            if kept == 0 || id != last {
                sorted.swap(kept, at);
                kept += 1;
            }
        }
        classes.truncate(kept);
        Ok(Self { classes, entries })
    }

    /// `len(ClassInformation)`: the classes, each id once.
    pub(crate) fn len(&self) -> usize {
        self.classes.len()
    }

    /// Every class's id, sorted.
    pub(crate) fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.classes.as_slice().iter().map(|c| c.id)
    }

    /// Every entry of every listing, in the order they were read.
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// `ClassInformation[class_id]`: the class's entries in their stored
    /// order, and the first one's place among [`Self::entry_count`]'s;
    /// `None` where UnityPy raises `KeyError`.
    pub(crate) fn get(&self, id: i32) -> Option<(usize, &[ClassEntry])> {
        let classes = self.classes.as_slice();
        let class = classes
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .and_then(|at| classes.get(at))?;
        let entries = self
            .entries
            .as_slice()
            .get(class.first..class.first.checked_add(class.len)?)?;
        Some((class.first, entries))
    }
}
