// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp and
// UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.

//! What both readers share to build a dict and to resolve a
//! `ReferencedObject`'s data: the linear `PyDict_SetItem` of plan 3.10
//! ("Linear dicts") and the ref-type scan's pieces.
//!
//! **Linear dicts.** A class's entries grow in a metered vector sized from
//! the compiled tree (its distinct child names that the read will set). A
//! class whose children repeat a name keeps a per-read scratch from each
//! name's `slot` to its entry's position, so a repeated name overwrites its
//! first entry in place (`PyDict_SetItem`: first position, last value) in
//! O(1), and a name a skipped child would have set first lands where the
//! first child actually read puts it. The scratch is a region of one
//! metered stack, cleared per class read.

use std::sync::Arc;

use crate::files::SerializedType;
use crate::limits::{Limits, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::{NodeId, Tree};
use crate::value::{ListIx, Map, Value, ValueBuilder};

use super::{Out, ReadDetail, ReadError, ReadErrorKind, charge_error};

/// No entry yet for a slot.
const NONE: u32 = u32::MAX;

/// One dict being read: its entries and its region of the slot scratch.
#[derive(Debug)]
pub(crate) struct Dict {
    pub(crate) entries: MeteredVec<(NodeId, Value)>,
    /// Start of this dict's region in the scratch; `None` when the class
    /// repeats no name and entries are only pushed.
    base: Option<usize>,
}

impl Dict {
    /// Starts a dict of `reserve` entries. A dict whose keys may repeat
    /// (`slots > 0`) takes a region of `slots` positions in `scratch`.
    pub(crate) fn new(
        builder: &ValueBuilder,
        scratch: &mut MeteredVec<u32>,
        reserve: usize,
        slots: usize,
        at: u64,
    ) -> Result<Self, ReadError> {
        let mut entries = builder.vec();
        entries
            .reserve_exact(reserve)
            .map_err(|e| charge_error(at, e))?;
        let base = if slots > 0 {
            let base = scratch.len();
            scratch.reserve(slots).map_err(|e| charge_error(at, e))?;
            for _ in 0..slots {
                scratch.push(NONE).map_err(|e| charge_error(at, e))?;
            }
            Some(base)
        } else {
            None
        };
        Ok(Self { entries, base })
    }

    /// The entry position of `slot`, if set.
    pub(crate) fn position(&self, scratch: &MeteredVec<u32>, slot: u32) -> Option<usize> {
        let base = self.base?;
        let at = scratch
            .as_slice()
            .get(base.saturating_add(slot as usize))
            .copied()?;
        (at != NONE).then_some(at as usize)
    }

    /// `PyDict_SetItem(dict, name, value)`: a key already set keeps its
    /// position and takes the value; a new one is appended, growing the
    /// entries by exactly one when they are full (a `ReferencedObject`'s
    /// data, which the read may skip, is not reserved ahead).
    pub(crate) fn set(
        &mut self,
        scratch: &mut MeteredVec<u32>,
        key: NodeId,
        slot: u32,
        value: Out,
        at: u64,
    ) -> Result<(), ReadError> {
        if let Some(pos) = self.position(scratch, slot) {
            value.set_entry(&mut self.entries, pos);
            return Ok(());
        }
        let pos = self.entries.len();
        if pos == self.entries.capacity() {
            self.entries
                .reserve_exact(1)
                .map_err(|e| charge_error(at, e))?;
        }
        value
            .push_entry(&mut self.entries, key)
            .map_err(|e| charge_error(at, e))?;
        if let Some(base) = self.base
            && let Some(s) = scratch
                .as_mut_slice()
                .get_mut(base.saturating_add(slot as usize))
        {
            *s = u32::try_from(pos).unwrap_or(NONE);
        }
        Ok(())
    }

    /// The finished dict; its scratch region is released.
    pub(crate) fn finish(
        self,
        builder: &ValueBuilder,
        scratch: &mut MeteredVec<u32>,
        list: ListIx,
    ) -> Out {
        if let Some(base) = self.base {
            scratch.truncate(base);
        }
        Out::Built(builder.map(list, self.entries))
    }
}

/// The trees a read's maps are keyed by, in [`crate::Parsed::lists`]
/// order: the root's, then each ref type's a `ReferencedObject` read
/// through. A mirror of the builder's lists table, so a reader can name a
/// map's keys (the `type` lookup of R14).
#[derive(Debug)]
pub(crate) struct Trees {
    trees: MeteredVec<Arc<Tree>>,
}

impl Trees {
    pub(crate) fn new(builder: &ValueBuilder, root: &Arc<Tree>) -> Result<Self, ReadError> {
        let mut trees = builder.vec();
        trees.push(root.clone()).map_err(|e| charge_error(0, e))?;
        Ok(Self { trees })
    }

    /// The list index of `tree`'s keys, adding it to the builder's lists
    /// (once per distinct tree) and to this mirror.
    pub(crate) fn index(
        &mut self,
        builder: &mut ValueBuilder,
        tree: &Arc<Tree>,
        at: u64,
    ) -> Result<ListIx, ReadError> {
        let ix = builder
            .list_index(tree.list())
            .map_err(|e| charge_error(at, e))?;
        if ix as usize == self.trees.len() {
            self.trees
                .push(tree.clone())
                .map_err(|e| charge_error(at, e))?;
        }
        Ok(ix)
    }

    /// The name of `key` in a map keyed by list `list`.
    pub(crate) fn name(&self, list: ListIx, key: NodeId) -> &[u8] {
        self.trees
            .as_slice()
            .get(list as usize)
            .map_or(&[], |t| t.list().name(key))
    }

    /// `map[name]`, a Python dict lookup by key string.
    pub(crate) fn field<'v>(&self, map: &'v Map, name: &[u8]) -> Option<&'v Value> {
        map.entries()
            .iter()
            .find(|(k, _)| self.name(map.list(), *k) == name)
            .map(|(_, v)| v)
    }
}

/// How the first child named `name` of `node` is keyed in its dict: its
/// key's slot.
pub(crate) fn slot_of(tree: &Tree, node: NodeId, name: &[u8]) -> Option<u32> {
    tree.list()
        .children(node)
        .iter()
        .find(|c| tree.list().name(**c) == name)
        .and_then(|c| tree.info(*c))
        .map(|i| i.slot)
}

/// The number of slots a dict of `node`'s children uses (its distinct names).
pub(crate) fn slot_count(tree: &Tree, node: NodeId) -> usize {
    tree.list()
        .children(node)
        .iter()
        .filter_map(|c| tree.info(*c))
        .map(|i| i.slot as usize + 1)
        .max()
        .unwrap_or(0)
}

/// A ref type's `(class, ns, asm)` equal to the three values: each must be
/// a Python `str` (a surrogate-escaped string compares equal exactly when
/// its bytes do), as `PyUnicode_Compare` (UnityPy TypeTreeHelper.cpp:663)
/// and the pure-Python `==` (UnityPy TypeTreeHelper.py:106) compare them;
/// any other object never matches.
pub(crate) fn names_match(t: &SerializedType, cls: &Value, ns: &Value, asm: &Value) -> bool {
    let eq = |v: &Value, s: &Option<RawStr>| match (v, s) {
        (Value::Str(a), Some(b)) => a.as_bytes() == b.as_bytes(),
        _ => false,
    };
    eq(cls, &t.class_name) && eq(ns, &t.namespace) && eq(asm, &t.assembly)
}

/// What a matched ref type gives the data child: its compiled tree, or no
/// tree (type trees disabled; UnityPy's `node is None`).
pub(crate) enum RefNode {
    /// Read the data with this tree.
    Tree(Arc<Tree>),
    /// No tree: the readers skip `data` (or, on the pure-Python array path,
    /// raise [`ReadDetail::RefTypeNoTree`]).
    NoTree,
}

/// The compiled tree of a matched ref type.
pub(crate) fn ref_tree(t: &SerializedType, limits: &Limits, at: u64) -> Result<RefNode, ReadError> {
    match t.tree(limits) {
        None => Ok(RefNode::NoTree),
        Some(Ok(tree)) => Ok(RefNode::Tree(tree)),
        Some(Err(e)) => Err(ReadError {
            kind: ReadErrorKind::Other,
            at,
            detail: ReadDetail::Tree(e),
        }),
    }
}
