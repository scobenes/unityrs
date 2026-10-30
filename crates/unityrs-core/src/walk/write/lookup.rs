//! How the writer finds each child's entry in a dict, `value[child.m_Name]`,
//! in steps linear in the children, as a Python dict finds it ([`Lookup`]),
//! and the name tables it builds for a miss, built once per dict
//! ([`NameTable`], [`KeptTables`]).

use std::collections::HashMap;
use std::sync::Arc;

use crate::limits::{Charge, Meter, MeteredVec, hash_table_cost};
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
/// same-named sibling, a caller's map) finds the name in a [`NameTable`],
/// built on the first miss: this `Lookup`'s own, or, inside a class whose
/// children repeat a name, the one [`KeptTables`] keeps for the dict, so a
/// dict handed to many siblings is indexed once. Every table is charged to
/// the value's meter.
pub(super) struct Lookup<'a> {
    map: &'a Map,
    /// Keyed by this node's children: read through it.
    native: bool,
    /// Slot to position, for a node whose children repeat a name.
    found: MeteredVec<u32>,
    cursor: usize,
    by_name: Option<NameTable<'a>>,
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

    /// `child`'s entry; a miss uses `kept`'s table for the dict when given.
    pub(super) fn get(
        &mut self,
        parsed: &'a Parsed,
        tree: &Tree,
        child: NodeId,
        kept: Option<&mut KeptTables<'a>>,
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
            (None, false) => {
                if let Some(k) = kept {
                    k.position(parsed, self.map, name)?
                } else {
                    if self.by_name.is_none() {
                        self.by_name = Some(NameTable::build(parsed, self.map)?);
                    }
                    self.by_name.as_ref().and_then(|t| t.position(name))
                }
            }
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
}

/// A dict's table from each name to its first entry's position: a Python
/// dict's lookup by key string.
pub(super) struct NameTable<'a> {
    first: HashMap<&'a [u8], u32>,
    _charge: Charge,
}

impl<'a> NameTable<'a> {
    /// Built over `map`'s entries, charged to the value's meter before it
    /// allocates ([`hash_table_cost`]).
    pub(super) fn build(parsed: &'a Parsed, map: &'a Map) -> Result<Self, WriteError> {
        let n = map.entries().len();
        let charge = parsed
            .charge()
            .meter()
            .charge(hash_table_cost(n, size_of::<(&[u8], u32)>()))
            .map_err(WriteError::Charge)?;
        let mut first = HashMap::with_capacity(n);
        for (i, (name, _)) in parsed.entries(map).enumerate() {
            steps(1);
            first
                .entry(name)
                .or_insert(u32::try_from(i).unwrap_or(UNSET));
        }
        Ok(Self {
            first,
            _charge: charge,
        })
    }

    /// The first entry named `name` (a [`Lookup`] counts the step).
    pub(super) fn position(&self, name: &[u8]) -> Option<usize> {
        self.first
            .get(name)
            .filter(|p| **p != UNSET)
            .map(|p| *p as usize)
    }
}

/// The [`NameTable`] of every dict that missed inside a class whose
/// children repeat a name (`dup_names`), by the dict's address, kept until
/// the outermost such class is written. A repeated name hands its one
/// entry to each same-named child, which writes it whole, so one dict, and
/// every dict inside it, is written once a sibling (claude B, review round
/// 2: `K` siblings over a dict of `N` entries built `K` tables of `N`);
/// keyed by address, each is indexed once. Outside such a class each dict
/// is written once, and a [`Lookup`]'s own table is dropped with it. The
/// map of tables is charged to the value's meter before it grows
/// ([`hash_table_cost`]).
pub(super) struct KeptTables<'a> {
    /// By the dict's address.
    tables: HashMap<usize, NameTable<'a>>,
    charge: Charge,
}

impl<'a> KeptTables<'a> {
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            tables: HashMap::new(),
            charge: Charge::zero(meter),
        }
    }

    /// What the map of `n` tables is charged.
    fn cost(n: usize) -> u64 {
        if n == 0 {
            return 0;
        }
        hash_table_cost(n, size_of::<(usize, NameTable<'_>)>())
    }

    /// The first entry of `map` named `name`, by `map`'s table, built (and
    /// the map of tables charged for it, before `insert` can grow it) on
    /// the dict's first miss.
    pub(super) fn position(
        &mut self,
        parsed: &'a Parsed,
        map: &'a Map,
        name: &[u8],
    ) -> Result<Option<usize>, WriteError> {
        let key = std::ptr::from_ref(map).addr();
        if let Some(t) = self.tables.get(&key) {
            return Ok(t.position(name));
        }
        let n = self.tables.len();
        let more = Self::cost(n + 1).saturating_sub(Self::cost(n));
        self.charge.grow(more).map_err(WriteError::Charge)?;
        let table = NameTable::build(parsed, map)?;
        let pos = table.position(name);
        self.tables.insert(key, table);
        Ok(pos)
    }
}
