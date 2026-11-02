// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp and
// UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.

//! What both readers share to build a dict and to resolve a
//! `ReferencedObject`'s data: the linear `PyDict_SetItem` of plan 3.10
//! ("Linear dicts") and the ref-type search ([`RefIndex`], which the writer
//! shares too).
//!
//! **Linear dicts.** A class's entries grow in a metered vector as its
//! children are read, as a Python dict grows as `PyDict_SetItem` fills it:
//! a first reservation of at most [`DICT_FIRST`] entries, then
//! geometrically, never past its children ([`Dict`]). A class whose
//! children repeat a name, and a `ReferencedObject` (whose `type` is looked
//! up by name), keep a table from each name's `slot` to its entry's
//! position, so a repeated name overwrites its first entry in place
//! (`PyDict_SetItem`: first position, last value) in O(1), and a name a
//! skipped child would have set first lands where the first child actually
//! read puts it. The table too grows as the children are set: a child's
//! slot is at most the number of children before it. So a read makes no
//! pass over a class's children before it reads them, and holds no entry or
//! slot the tree sizes before its bytes are read (review round 7: a class
//! reserved one entry a child before reading any, and every level of a
//! `ReferencedObject` nested in its ref type held its own).

use std::collections::HashMap;
use std::sync::Arc;

use crate::files::SerializedType;
use crate::limits::{Charge, ChargeError, Limits, Meter, MeteredVec, hash_table_cost};
use crate::raw::RawStr;
use crate::typetree::{NodeId, Tree};
use crate::value::{ListIx, Map, Value, ValueBuilder};

use super::{
    OpenWork, Out, ReadDetail, ReadError, ReadErrorKind, RefTypes, VALUE_WORK, Work, charge_error,
};
use crate::error::LimitError;

/// No entry yet for a slot.
const NONE: u32 = u32::MAX;

#[cfg(test)]
std::thread_local! {
    /// Map steps this thread took: each slot a table of positions grows
    /// by, each entry set or looked up, each entry a growth may copy, each
    /// entry a lookup by name passes, each ref type or node list a search
    /// passes; the writer's lookups too (the linear-work tests).
    pub(super) static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts `n` map steps (tests only).
#[inline]
pub(crate) fn steps(n: usize) {
    #[cfg(test)]
    STEPS.with(|s| s.set(s.get().saturating_add(n as u64)));
    #[cfg(not(test))]
    let _ = n;
}

#[cfg(test)]
std::thread_local! {
    /// Loop turns this thread's walks took: each child a class's or a
    /// `ReferencedObject`'s loop visits (read, written or skipped), in both
    /// readers and the writer, and each element the writer's array loop
    /// visits (the work tests: an instrument apart from the count that
    /// bounds them, [`crate::limits::Limits::max_values_per_object`]).
    pub(super) static VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts one loop turn (tests only).
#[inline]
pub(crate) fn visit() {
    #[cfg(test)]
    VISITS.with(|s| s.set(s.get().saturating_add(1)));
}

/// The most entries a dict reserves before its first child is read: one
/// for each child up to this many. A dict grows past it geometrically as its
/// children are set, so a read holds no more than this many entries a dict
/// that it has not filled, beyond as many again as it has filled (review
/// round 7: every `ReferencedObject` level re-entering a ref type of `k`
/// children held `k` entries reserved while the next level was read,
/// gigabytes for a few kilobytes of data).
pub const DICT_FIRST: usize = 16;

/// One dict being read: its entries, and the position of each name's entry
/// when its keys may repeat.
#[derive(Debug)]
pub(crate) struct Dict {
    pub(crate) entries: MeteredVec<(NodeId, Value)>,
    /// Each slot's entry position ([`NONE`]: not set yet), for a dict whose
    /// keys may repeat or which is looked up by name; `None` when its
    /// children's names are all distinct and its entries are only pushed.
    /// It covers the slots set so far, and a child's slot is at most the
    /// number of children before it, so it holds no more than one slot a
    /// child read.
    slots: Option<MeteredVec<u32>>,
    /// The most entries it can come to: one a child.
    most: usize,
}

impl Dict {
    /// Starts the dict of a node of `children` children, reserving at most
    /// [`DICT_FIRST`] entries; `keyed` keeps each name's position (a class
    /// whose children repeat a name, a `ReferencedObject`).
    pub(crate) fn new(
        builder: &ValueBuilder,
        children: usize,
        keyed: bool,
        at: u64,
    ) -> Result<Self, ReadError> {
        let mut entries = builder.vec();
        entries
            .reserve_exact(children.min(DICT_FIRST))
            .map_err(|e| charge_error(at, e))?;
        Ok(Self {
            entries,
            slots: keyed.then(|| builder.vec()),
            most: children,
        })
    }

    /// The entry position of `slot`, if set.
    pub(crate) fn position(&self, slot: u32) -> Option<usize> {
        let at = self
            .slots
            .as_ref()?
            .as_slice()
            .get(slot as usize)
            .copied()?;
        (at != NONE).then_some(at as usize)
    }

    /// `PyDict_SetItem(dict, name, value)`: a key already set keeps its
    /// position and takes the value; a new one is appended. Entries that are
    /// full grow geometrically, never past the dict's children, so growths
    /// that move the entries copy them a bounded number of times (review
    /// round 6's sweep: growing by exactly one, a dict reserved short relied
    /// on the allocator's growing it in place). A slot past the table's end
    /// grows the table to it, each slot a step of the read's work
    /// ([`Work::steps`]), counted first.
    pub(crate) fn set(
        &mut self,
        work: &mut Work<'_>,
        key: NodeId,
        slot: u32,
        value: Out,
        at: u64,
    ) -> Result<(), ReadError> {
        steps(1);
        if let Some(pos) = self.position(slot) {
            // The value it replaces was built and is not kept.
            work.discard();
            value.set_entry(&mut self.entries, pos);
            return Ok(());
        }
        let pos = self.entries.len();
        if pos == self.entries.capacity() {
            // What the growth may copy (the linear-work tests).
            steps(pos);
            self.entries
                .reserve_within(1, self.most)
                .map_err(|e| charge_error(at, e))?;
        }
        value
            .push_entry(&mut self.entries, key)
            .map_err(|e| charge_error(at, e))?;
        if let Some(slots) = &mut self.slots {
            let slot = slot as usize;
            let more = (slot + 1).saturating_sub(slots.len());
            if more > 0 {
                work.steps(more, at)?;
                steps(more);
                slots.reserve(more).map_err(|e| charge_error(at, e))?;
                for _ in 0..more {
                    slots.push(NONE).map_err(|e| charge_error(at, e))?;
                }
            }
            if let Some(s) = slots.as_mut_slice().get_mut(slot) {
                *s = u32::try_from(pos).unwrap_or(NONE);
            }
        }
        Ok(())
    }

    /// The finished dict, with no spare capacity: a dict whose children set
    /// fewer entries than it grew to (a repeated name, a skipped child) gives
    /// the rest back, charged before it moves
    /// ([`MeteredVec::shrink_to_fit`]).
    pub(crate) fn finish(
        mut self,
        builder: &ValueBuilder,
        list: ListIx,
        at: u64,
    ) -> Result<Out, ReadError> {
        self.entries
            .shrink_to_fit()
            .map_err(|e| charge_error(at, e))?;
        Ok(Out::Built(builder.map(list, self.entries)))
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

    /// `map[name]`, a Python dict lookup by key string: each entry it
    /// passes is a step of the open's work, counted before it is compared
    /// (a key whose length differs from `name`'s is passed without reading
    /// its bytes).
    pub(crate) fn field<'v>(
        &self,
        map: &'v Map,
        name: &[u8],
        work: &mut OpenWork<'_>,
    ) -> Result<Option<&'v Value>, LimitError> {
        for (k, v) in map.entries() {
            steps(1);
            work.steps(1)?;
            if self.name(map.list(), *k) == name {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }
}

/// A ref-type search's refusal: the table's memory, or the open's work.
#[derive(Debug)]
pub(crate) enum FindError {
    /// The meter or the budget refused the table.
    Charge(ChargeError),
    /// The open's work passed its allowance ([`OpenWork`]).
    Work(LimitError),
}

impl From<LimitError> for FindError {
    fn from(l: LimitError) -> Self {
        Self::Work(l)
    }
}

/// The bytes of a ref type's three names.
fn names_len(t: &SerializedType) -> usize {
    [&t.class_name, &t.namespace, &t.assembly]
        .into_iter()
        .map(|n| n.as_ref().map_or(0, |n| n.as_bytes().len()))
        .sum()
}

/// A ref type's `(class, ns, asm)` equal to the three values: each must be
/// a Python `str` (a surrogate-escaped string compares equal exactly when
/// its bytes do), as `PyUnicode_Compare` (UnityPy TypeTreeHelper.cpp:663)
/// and the pure-Python `==` (UnityPy TypeTreeHelper.py:106) compare them;
/// any other object never matches.
fn names_match(t: &SerializedType, cls: &Value, ns: &Value, asm: &Value) -> bool {
    let eq = |v: &Value, s: &Option<RawStr>| match (v, s) {
        (Value::Str(a), Some(b)) => a.as_bytes() == b.as_bytes(),
        _ => false,
    };
    eq(cls, &t.class_name) && eq(ns, &t.namespace) && eq(asm, &t.assembly)
}

/// `get_ref_type_node`'s search: the *first* ref type whose class,
/// namespace and assembly equal the three values ([`names_match`]; UnityPy
/// TypeTreeHelper.cpp:644-670, TypeTreeHelper.py:105-107), found in steps
/// linear in the list plus the lookups of one read or one write. UnityPy
/// scans the list for every lookup, so an object of `n` references into
/// `n` ref types costs it `n`² comparisons (claude X, review round 2).
/// Here each lookup scans as UnityPy does until this read's or write's
/// scans have passed the list's length; then, for a list longer than
/// [`SCANNED_REFS`], a table from each `(class, ns, asm)` to its first
/// index is built, once, charged to the meter before it allocates, and
/// every later lookup is one probe. A read or a write so costs at most
/// twice what UnityPy's own scans cost it, and at most three steps a ref
/// type plus one a lookup (or [`SCANNED_REFS`] a lookup).
///
/// That bounds one read or write; the open's work bounds them all: the
/// index is one read's or write's, and every object of a file searches the
/// file's one list again, so K objects over R ref types cost K x R steps
/// that their values do not count (the sweep of fix pass 5). Each ref type
/// a scan compares or the table indexes counts [`VALUE_WORK`] and the bytes
/// of its three names, what a comparison or a hash reads of it at most, and
/// each probe [`VALUE_WORK`] and the bytes of the key it hashes, on the
/// read's or the write's [`OpenWork`], before the step is taken.
pub(crate) struct RefIndex<'a> {
    list: &'a [SerializedType],
    /// The ref types the scans so far compared.
    scanned: usize,
    table: Option<RefTable<'a>>,
}

/// A list this long or shorter is only ever scanned: a lookup costs at most
/// as many steps as a probe's hashing.
const SCANNED_REFS: usize = 8;

/// [`RefIndex`]'s table.
struct RefTable<'a> {
    first: HashMap<[&'a [u8]; 3], usize>,
    /// The longest class, namespace and assembly: a longer value matches
    /// nothing and is not hashed (a scan's comparison checks lengths first).
    longest: [usize; 3],
    _charge: Charge,
}

impl<'a> RefIndex<'a> {
    /// Over `refs`' list (empty for no list: the callers check that first).
    pub(crate) fn new(refs: RefTypes<'a>) -> Self {
        let list = match refs {
            RefTypes::List(l) => l,
            RefTypes::NoFile | RefTypes::Absent => &[],
        };
        Self {
            list,
            scanned: 0,
            table: None,
        }
    }

    /// The first ref type whose names equal `(cls, ns, asm)`, the table
    /// charged to `meter` when this lookup builds it, each step counted on
    /// `work` before it is taken.
    pub(crate) fn find(
        &mut self,
        meter: &Arc<Meter>,
        work: &mut OpenWork<'_>,
        cls: &Value,
        ns: &Value,
        asm: &Value,
    ) -> Result<Option<&'a SerializedType>, FindError> {
        let (Value::Str(c), Value::Str(n), Value::Str(a)) = (cls, ns, asm) else {
            // A value that is not a `str` matches no ref type.
            return Ok(None);
        };
        if self.table.is_none() && self.scanned >= self.list.len() && self.list.len() > SCANNED_REFS
        {
            self.table = Some(RefTable::build(self.list, meter, work)?);
        }
        if let Some(t) = &self.table {
            steps(1);
            let key = [c.as_bytes(), n.as_bytes(), a.as_bytes()];
            if key.iter().zip(t.longest).any(|(k, l)| k.len() > l) {
                work.add(VALUE_WORK)?;
                return Ok(None);
            }
            work.add(VALUE_WORK.saturating_add(as_u64(key.iter().map(|k| k.len()).sum())))?;
            let first: &HashMap<[&[u8]; 3], usize> = &t.first;
            return Ok(first.get(&key).and_then(|i| self.list.get(*i)));
        }
        for (i, r) in self.list.iter().enumerate() {
            steps(1);
            work.add(VALUE_WORK.saturating_add(as_u64(names_len(r))))?;
            if names_match(r, cls, ns, asm) {
                self.scanned = self.scanned.saturating_add(i + 1);
                return Ok(Some(r));
            }
        }
        self.scanned = self.scanned.saturating_add(self.list.len());
        Ok(None)
    }
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl<'a> RefTable<'a> {
    /// Every ref type with all three names, first index kept, charged
    /// before it allocates ([`hash_table_cost`]), each ref type counted on
    /// `work` before it is indexed.
    fn build(
        list: &'a [SerializedType],
        meter: &Arc<Meter>,
        work: &mut OpenWork<'_>,
    ) -> Result<Self, FindError> {
        let n = list.len();
        let charge = meter
            .charge(hash_table_cost(n, size_of::<([&[u8]; 3], usize)>()))
            .map_err(FindError::Charge)?;
        let mut first = HashMap::with_capacity(n);
        let mut longest = [0; 3];
        for (i, r) in list.iter().enumerate() {
            steps(1);
            work.add(VALUE_WORK.saturating_add(as_u64(names_len(r))))?;
            let (Some(c), Some(ns), Some(a)) = (&r.class_name, &r.namespace, &r.assembly) else {
                continue;
            };
            let key = [c.as_bytes(), ns.as_bytes(), a.as_bytes()];
            for (l, k) in longest.iter_mut().zip(key) {
                *l = (*l).max(k.len());
            }
            first.entry(key).or_insert(i);
        }
        Ok(Self {
            first,
            longest,
            _charge: charge,
        })
    }
}

/// What a matched ref type gives the data child: its compiled tree, or no
/// tree (type trees disabled; UnityPy's `node is None`).
#[derive(Clone)]
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;

    use super::STEPS;
    use crate::limits::{Limits, Meter};
    use crate::stream::Endian;
    use crate::typetree::{NodeList, NodeRow, Tree};
    use crate::value::Value;
    use crate::walk::{ReadCtx, ReadOptions, RefTypes, boost, python};

    /// Plan 3.10, linear dicts: a class of 2^16 children whose last 4,096
    /// repeat the first 4,096 names reads in map steps linear in the
    /// children (counted, not timed, the growth of its entries and of its
    /// table of positions included; a dict that looked each name up among
    /// its entries would
    /// take some 2^31), each repeated name keeping its
    /// first position and taking its last value, from both readers.
    #[test]
    fn late_repeats_read_in_linear_map_steps() {
        const N: u32 = 1 << 16;
        const REPEATS: u32 = 4096;
        let distinct = N - REPEATS;
        let mut rows = vec![NodeRow::new(0, "Base", "Base")];
        for i in 0..N {
            let name = format!("n{}", if i < distinct { i } else { i - distinct });
            rows.push(NodeRow::new(1, "int", name.as_str()));
        }
        let tree = Tree::compile(
            Arc::new(NodeList::trusted(rows).unwrap()),
            &Limits::default(),
        )
        .unwrap();
        let data: Vec<u8> = (0..N).flat_map(u32::to_le_bytes).collect();
        let data = Bytes::from(data);
        let options = ReadOptions::default();
        let meter = Meter::unlimited();
        let ctx = ReadCtx {
            endian: Endian::Little,
            ref_types: RefTypes::List(&[]),
            options: &options,
            meter: &meter,
        };
        for reader in ["boost", "python"] {
            STEPS.with(|s| s.set(0));
            let p = match reader {
                "boost" => boost::read(&data, &tree, &ctx),
                _ => python::read(&data, 0, &tree, &ctx),
            }
            .unwrap();
            let steps = STEPS.with(std::cell::Cell::get);
            assert!(
                steps <= 6 * u64::from(N),
                "{reader}: {steps} map steps for {N} children"
            );
            let Value::Dict(m) = p.value() else {
                panic!("{reader}: not a dict")
            };
            assert_eq!(m.len(), distinct as usize, "{reader}");
            for (i, (name, v)) in p.entries(m).enumerate() {
                let i = u32::try_from(i).unwrap();
                assert_eq!(name, format!("n{i}").as_bytes(), "{reader}: position {i}");
                let last = if i < REPEATS { distinct + i } else { i };
                assert_eq!(v, &Value::Int(i64::from(last)), "{reader}: n{i}");
            }
        }
    }
}
