//! How the writer finds each child's entry in a dict, `value[child.m_Name]`,
//! in steps linear in the children, as a Python dict finds it ([`Lookup`]).

use std::collections::HashMap;
use std::sync::Arc;

use crate::limits::{Charge, HASH_TABLE_SHARES, MeteredVec, heap_cost};
use crate::typetree::{NodeId, Tree};
use crate::value::{Map, Parsed, Value};
use crate::walk::WriteError;
use crate::walk::dict::{slot_count, steps};

/// Where each child of a class or `ReferencedObject` node finds its entry in
/// a dict, `value[child.m_Name]`, in steps linear in the children, as a
/// Python dict finds it. A cursor walks the entries in step with the
/// children (a read sets them in child order). A dict read through this
/// node also keeps a per-node table from a name's slot to its position, for
/// a child whose name an earlier child took. Any other miss (a dict keyed by
/// another list: a `nodes=` override, a dict a repeated name hands a
/// same-named sibling, a caller's map) finds the name in a table from each
/// name to its first entry, built on the first miss, once per dict. Both
/// tables are charged to the value's meter.
pub(super) struct Lookup<'a> {
    map: &'a Map,
    /// Keyed by this node's children: read through it.
    native: bool,
    /// Slot to position, for a node whose children repeat a name.
    found: MeteredVec<u32>,
    cursor: usize,
    by_name: Option<(HashMap<&'a [u8], u32>, Charge)>,
}

const UNSET: u32 = u32::MAX;

impl<'a> Lookup<'a> {
    pub(super) fn new(
        parsed: &'a Parsed,
        tree: &Tree,
        node: NodeId,
        map: &'a Map,
    ) -> Result<Self, WriteError> {
        let kids = tree.list().children(node);
        // (A dict read through a same-named sibling, which a repeated name
        // hands this node, is keyed by that sibling's children.)
        let native = usize::try_from(map.list())
            .ok()
            .and_then(|i| parsed.lists().get(i))
            .is_some_and(|l| Arc::ptr_eq(l, tree.list()))
            && map
                .entries()
                .first()
                .is_none_or(|(k, _)| kids.binary_search_by_key(&k.0, |c| c.0).is_ok());
        let slots = if native { slot_count(tree, node) } else { 0 };
        let mut found: MeteredVec<u32> = MeteredVec::new(parsed.charge().meter());
        if slots < kids.len() {
            found.reserve_exact(slots).map_err(WriteError::Charge)?;
            for _ in 0..slots {
                found.push(UNSET).map_err(WriteError::Charge)?;
            }
        }
        Ok(Self {
            map,
            native,
            found,
            cursor: 0,
            by_name: None,
        })
    }

    /// `child`'s entry.
    pub(super) fn get(
        &mut self,
        parsed: &'a Parsed,
        tree: &Tree,
        child: NodeId,
    ) -> Result<Option<&'a Value>, WriteError> {
        let entries = self.map.entries();
        let name = tree.list().name(child);
        steps(1);
        let at_cursor = entries.get(self.cursor).is_some_and(|(k, _)| {
            if self.native {
                Some(*k) == tree.info(child).map(|i| i.key)
            } else {
                parsed.key_name(self.map, *k) == name
            }
        });
        let slot = tree
            .info(child)
            .filter(|_| self.native)
            .map(|i| i.slot as usize);
        let known = slot
            .and_then(|s| self.found.as_slice().get(s).copied())
            .filter(|p| *p != UNSET)
            .map(|p| p as usize);
        let pos = match (known, at_cursor) {
            (Some(p), _) => Some(p),
            (None, true) => Some(self.cursor),
            (None, false) => self.by_name(parsed)?.get(name).map(|p| *p as usize),
        };
        if let (Some(p), Some(f)) = (pos, slot.and_then(|s| self.found.as_mut_slice().get_mut(s))) {
            *f = u32::try_from(p).unwrap_or(UNSET);
        }
        if let Some(p) = pos
            && p >= self.cursor
        {
            self.cursor = p + 1;
        }
        Ok(pos.and_then(|p| entries.get(p)).map(|(_, v)| v))
    }

    /// Each name's first entry: built once, charged before it allocates
    /// ([`HASH_TABLE_SHARES`] buckets a name, and a table's fixed part).
    fn by_name(&mut self, parsed: &'a Parsed) -> Result<&HashMap<&'a [u8], u32>, WriteError> {
        let built = if let Some(b) = self.by_name.take() {
            b
        } else {
            let n = self.map.entries().len();
            let bucket = (size_of::<(&[u8], u32)>() + 1) as u64;
            let bytes = (n.max(2) as u64)
                .saturating_mul(HASH_TABLE_SHARES * bucket)
                .saturating_add(16);
            let charge = parsed
                .charge()
                .meter()
                .charge(heap_cost(bytes))
                .map_err(WriteError::Charge)?;
            let mut first = HashMap::with_capacity(n);
            for (i, (name, _)) in parsed.entries(self.map).enumerate() {
                steps(1);
                first
                    .entry(name)
                    .or_insert(u32::try_from(i).unwrap_or(UNSET));
            }
            (first, charge)
        };
        Ok(&self.by_name.insert(built).0)
    }
}
