// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Typetree node rows, and UnityPy's rule for building a tree from them.
//!
//! A [`NodeRow`] holds one node as stored: UnityPy's `TypeTreeNode` fields,
//! with `None` where the format has no such field. [`NodeList::from_rows`]
//! builds the tree exactly as UnityPy's `TypeTreeNode.parse_blob` and
//! `from_list` do (`helpers/TypeTreeNode.py:131-154`, `:156-188`):
//!
//! - under a hidden root at level -1, a row whose level is above the previous
//!   row's becomes the previous row's child, however far above;
//! - a row at the same level becomes its sibling;
//! - a row below it pops the parent stack while the row's level is at or below
//!   the parent's, and popping an empty stack raises (`IndexError`);
//! - only the hidden root's first child is kept: a later top-level row and
//!   everything after it are dropped from the tree, though they are still
//!   processed and can still raise.
//!
//! The kept rows are always a prefix of the input, and their order is a
//! pre-order of the tree, so [`NodeList::rows`] is the canonical dump's
//! `nodes` list (`docs/CANONICAL_DUMP.md` section 4.2). Old-format trees
//! (`TypeTreeNode.parse`, explicit child counts) are emitted by the parser as
//! pre-order rows with `level` = depth, which this rule rebuilds unchanged.
//!
//! **Memory.** The rows arrive in a [`MeteredVec`] the parser grew on the
//! input's meter, and every table [`NodeList::from_rows`] builds is charged
//! to the same meter before it is allocated, with no allocation per row. The
//! list keeps the charges, so its rows cost the input until the last `Arc`
//! of it drops (a [`crate::Parsed`] holds its lists).
//!
//! Owner: the scaffold (implemented). Package B compiles a `NodeList` into a
//! [`super::Tree`]; package C produces rows from files.

use std::sync::Arc;

use crate::error::LimitError;
use crate::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};
use crate::raw::RawStr;

/// One typetree node as stored. UnityPy's `TypeTreeNode` fields.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NodeRow {
    /// `m_Level`: as stored (blob `u8`), or the depth for old-format trees.
    pub level: i32,
    /// `m_Type`.
    pub ty: RawStr,
    /// `m_Name`.
    pub name: RawStr,
    /// `m_ByteSize`.
    pub byte_size: i32,
    /// `m_Version`: blob `i16` (`hBBIIiii`), old-format `i32`.
    pub version: i32,
    /// `m_TypeFlags`; `None` for TPK nodes (`helpers/Tpk.py`).
    pub type_flags: Option<i32>,
    /// `m_VariableCount`: old format version 2 only.
    pub variable_count: Option<i32>,
    /// `m_Index`, as stored, never recomputed.
    pub index: Option<i32>,
    /// `m_MetaFlag`; `None` in old format version 3. `& 0x4000` aligns.
    /// An `i64`, so that both stored widths keep UnityPy's value: a blob or
    /// old-format tree stores an `i32` (`hBBIIiii`, `read_int`), widened
    /// with its sign; a TPK node stores a `u32` (`Tpk.py:361`, `Struct
    /// ("<HHihbIH")`), widened without one, so a flag of 2^31 or more is the
    /// large positive int UnityPy holds (A request 3).
    pub meta_flag: Option<i64>,
    /// `m_RefTypeHash`: blob format 19 and up.
    pub ref_type_hash: Option<u64>,
}

impl NodeRow {
    /// A row with a level, a type and a name; the other fields as UnityPy's
    /// `from_list` patches them (`m_ByteSize` 0, `m_Version` 0, the rest `None`).
    #[must_use]
    pub fn new(level: i32, ty: impl Into<RawStr>, name: impl Into<RawStr>) -> Self {
        Self {
            level,
            ty: ty.into(),
            name: name.into(),
            ..Self::default()
        }
    }

    /// The same row with `m_MetaFlag` set.
    #[must_use]
    pub fn with_meta_flag(mut self, meta_flag: impl Into<i64>) -> Self {
        self.meta_flag = Some(meta_flag.into());
        self
    }

    /// Whether `m_MetaFlag & 0x4000` asks for 4-byte alignment after the value.
    #[must_use]
    pub fn aligned(&self) -> bool {
        self.meta_flag.is_some_and(|m| m & 0x4000 != 0)
    }
}

/// A node's index in its [`NodeList`] (pre-order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The index as `usize`.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Why rows do not make a tree.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TreeError {
    /// No rows (UnityPy: `IndexError` on the hidden root's first child).
    #[error("typetree has no nodes")]
    Empty,
    /// A row's level pops past the hidden root (UnityPy: `IndexError`).
    #[error("typetree row {row}: level {level} leaves the tree")]
    LevelUnderflow {
        /// The row's index in the input.
        row: usize,
        /// Its level.
        level: i32,
    },
    /// More rows than a `u32` indexes.
    #[error("typetree has {0} nodes")]
    TooManyNodes(usize),
    /// A compiled-tree bound was exceeded: its depth, or the open's work
    /// allowance, which the names a compile reads count toward
    /// ([`crate::LimitKind::DecodeWork`]: [`super::Tree::compile`]).
    #[error(transparent)]
    Limit(#[from] LimitError),
    /// The input's meter or the budget refused the tree's memory.
    #[error(transparent)]
    Charge(#[from] ChargeError),
}

impl crate::once::CellError for TreeError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Charge(c) => c.is_transient(),
            Self::Limit(l) => l.kind() == crate::LimitKind::InputMemory,
            Self::Empty | Self::LevelUnderflow { .. } | Self::TooManyNodes(_) => false,
        }
    }

    fn is_meter_refusal(&self) -> bool {
        match self {
            Self::Charge(c) => c.is_meter_refusal(),
            Self::Limit(l) => l.kind() == crate::LimitKind::InputMemory,
            Self::Empty | Self::LevelUnderflow { .. } | Self::TooManyNodes(_) => false,
        }
    }

    fn heap_bytes(&self) -> u64 {
        0
    }

    fn work_refused(limit: LimitError) -> Self {
        Self::Limit(limit)
    }
}

/// The kept tree of a typetree, as pre-order rows with their child lists.
/// Not `Clone`: it is shared as `Arc<NodeList>`, and owns the charge for
/// its rows.
#[derive(Debug)]
pub struct NodeList {
    rows: Vec<NodeRow>,
    /// `child_start[i]..child_start[i + 1]` indexes `children` for node `i`.
    child_start: Vec<u32>,
    children: Vec<NodeId>,
    /// The three tables' allocations, on the input's meter.
    charge: Charge,
}

impl PartialEq for NodeList {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
            && self.child_start == other.child_start
            && self.children == other.children
    }
}

impl Eq for NodeList {}

fn to_u32(i: usize) -> u32 {
    // Callers have checked the row count against `u32::MAX`.
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The parent stack of UnityPy's rule over a stack-depth bound: `None` is
/// the hidden root.
const ROOT: u32 = u32::MAX;

impl NodeList {
    /// Builds the tree by UnityPy's rule (module docs). The input is every row
    /// the file stores, in order, grown by the parser on the input's meter;
    /// the list's own tables are charged to that meter before they exist.
    pub fn from_rows(rows: MeteredVec<NodeRow>) -> Result<Self, TreeError> {
        let meter = rows.charge().meter().clone();
        let (mut rows, mut charge) = rows.into_parts();
        // The `Arc` the list is kept in (it always is), charged with it.
        charge.grow(heap_cost(
            u64::try_from(std::mem::size_of::<Self>() + 16).unwrap_or(u64::MAX),
        ))?;
        if rows.is_empty() {
            return Err(TreeError::Empty);
        }
        if u32::try_from(rows.len()).is_err() {
            return Err(TreeError::TooManyNodes(rows.len()));
        }
        let level_of = |rows: &[NodeRow], node: u32| -> i64 {
            if node == ROOT {
                return -1;
            }
            rows.get(node as usize).map_or(-1, |r| i64::from(r.level))
        };
        // Pass 1: each kept row's parent, by UnityPy's stack rule.
        let mut stack: MeteredVec<u32> = MeteredVec::new(&meter);
        stack.push(ROOT)?;
        let mut parents: MeteredVec<u32> = MeteredVec::with_capacity(&meter, rows.len())?;
        let mut parent = ROOT;
        let mut prev = ROOT;
        let mut kept_end: Option<usize> = None;
        for (i, row) in rows.iter().enumerate() {
            let level = i64::from(row.level);
            let prev_level = level_of(&rows, prev);
            if level > prev_level {
                stack.push(parent)?;
                parent = prev;
            } else if level < prev_level {
                while level <= level_of(&rows, parent) {
                    let depth = stack.len();
                    let Some(&up) = stack.as_slice().last() else {
                        return Err(TreeError::LevelUnderflow {
                            row: i,
                            level: row.level,
                        });
                    };
                    stack.truncate(depth - 1);
                    parent = up;
                }
            }
            if parent == ROOT && i > 0 && kept_end.is_none() {
                kept_end = Some(i);
            }
            if kept_end.is_none() {
                parents.push(parent)?;
            }
            prev = to_u32(i);
        }
        drop(stack);
        rows.truncate(kept_end.unwrap_or(rows.len()));
        let n = rows.len();

        // Pass 2: child lists by counting, so no allocation per row.
        let mut child_start: MeteredVec<u32> = MeteredVec::with_capacity(&meter, n + 1)?;
        for _ in 0..=n {
            child_start.push(0)?;
        }
        for &p in parents.as_slice().iter().skip(1) {
            if let Some(c) = child_start.as_mut_slice().get_mut(p as usize + 1) {
                *c += 1;
            }
        }
        let mut sum = 0;
        for c in child_start.as_mut_slice() {
            sum += *c;
            *c = sum;
        }
        // Now `child_start[p + 1]` is the end of `p`'s list; fill each list
        // from its start, using a cursor per node.
        let mut cursor: MeteredVec<u32> = MeteredVec::with_capacity(&meter, n)?;
        cursor.extend_from_slice(child_start.as_slice().get(..n).unwrap_or_default())?;
        let mut children: MeteredVec<NodeId> =
            MeteredVec::with_capacity(&meter, n.saturating_sub(1))?;
        for _ in 1..n {
            children.push(NodeId(0))?;
        }
        for (i, &p) in parents.as_slice().iter().enumerate().skip(1) {
            if let Some(at) = cursor.as_mut_slice().get_mut(p as usize) {
                if let Some(slot) = children.as_mut_slice().get_mut(*at as usize) {
                    *slot = NodeId(to_u32(i));
                }
                *at += 1;
            }
        }
        drop((cursor, parents));
        let (child_start, c1) = child_start.into_parts();
        let (children, c2) = children.into_parts();
        charge.absorb(c1);
        charge.absorb(c2);
        Ok(Self {
            rows,
            child_start,
            children,
            charge,
        })
    }

    /// Builds the tree from rows that are not an input's (a TPK's, a
    /// caller's `nodes=` list, a test's), on an unlimited meter.
    pub fn trusted(rows: Vec<NodeRow>) -> Result<Self, TreeError> {
        Self::from_rows(MeteredVec::from_vec(rows, &Meter::unlimited())?)
    }

    /// The charge for the list's tables; [`Charge::meter`] is the meter a
    /// tree compiled from it charges ([`super::Tree::compile`]).
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// The meter the list was built on.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The root (the hidden root's first child).
    #[must_use]
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// The kept rows in pre-order: the canonical dump's `nodes`.
    #[must_use]
    pub fn rows(&self) -> &[NodeRow] {
        &self.rows
    }

    /// Number of kept nodes (never 0).
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Always false: a `NodeList` has a root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// A node's row.
    #[must_use]
    pub fn row(&self, id: NodeId) -> Option<&NodeRow> {
        self.rows.get(id.index())
    }

    /// A node's children, in order. Empty for a leaf or an unknown id.
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        let i = id.index();
        match (self.child_start.get(i), self.child_start.get(i + 1)) {
            (Some(&a), Some(&b)) => self.children.get(a as usize..b as usize).unwrap_or(&[]),
            _ => &[],
        }
    }

    /// A node's `m_Name` bytes (empty for an unknown id).
    #[must_use]
    pub fn name(&self, id: NodeId) -> &[u8] {
        self.row(id).map_or(&[], |r| r.name.as_bytes())
    }

    /// A node's `m_Type` bytes (empty for an unknown id).
    #[must_use]
    pub fn type_name(&self, id: NodeId) -> &[u8] {
        self.row(id).map_or(&[], |r| r.ty.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{NodeId, NodeList, NodeRow, TreeError};
    use crate::budget::{Reservation, Unbounded};
    use crate::limits::{ChargeError, Meter, MeteredVec};

    fn rows(levels: &[i32]) -> MeteredVec<NodeRow> {
        let rows = levels
            .iter()
            .enumerate()
            .map(|(i, l)| NodeRow::new(*l, "int", format!("n{i}").as_str()))
            .collect();
        MeteredVec::from_vec(rows, &Meter::unlimited()).unwrap()
    }

    #[test]
    fn the_list_is_charged_and_returns_its_charge_when_dropped() {
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let mut v = MeteredVec::new(&meter);
        for l in [0, 1, 2, 2, 1] {
            v.push(NodeRow::new(l, "int", "x")).unwrap();
        }
        let list = NodeList::from_rows(v).unwrap();
        assert!(meter.used() >= u64::try_from(5 * std::mem::size_of::<NodeRow>()).unwrap());
        assert_eq!(meter.used(), list.charge().bytes(), "nothing else is held");
        drop(list);
        assert_eq!(meter.used(), 0);

        // A meter with no room left for the child tables refuses, and holds
        // nothing for the rows after.
        let mut v = MeteredVec::new(&meter);
        v.push(NodeRow::new(0, "int", "x")).unwrap();
        v.push(NodeRow::new(1, "int", "y")).unwrap();
        let fill = meter.charge(meter.limit() - meter.used()).unwrap();
        let e = NodeList::from_rows(v).unwrap_err();
        assert!(
            matches!(e, TreeError::Charge(ChargeError::Limit(_))),
            "{e:?}"
        );
        assert_eq!(meter.used(), fill.bytes());
        drop(fill);
        assert_eq!(meter.used(), 0);
    }

    #[test]
    fn a_level_jump_nests_under_the_previous_row() {
        // CANONICAL_DUMP 4.2: rows at levels 0, 2, 1 give a root with two children.
        let t = NodeList::from_rows(rows(&[0, 2, 1])).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.children(t.root()), &[NodeId(1), NodeId(2)]);
        assert_eq!(t.rows()[1].level, 2, "levels stay as stored");
    }

    #[test]
    fn a_later_top_level_row_and_the_rest_are_dropped() {
        let t = NodeList::from_rows(rows(&[0, 1, 0, 1, 2])).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.children(t.root()), &[NodeId(1)]);
    }

    #[test]
    fn a_dropped_row_can_still_raise() {
        // UnityPy keeps running the stack rule over dropped rows: -5 pops past the root.
        assert_eq!(
            NodeList::from_rows(rows(&[0, 1, 0, -5])),
            Err(TreeError::LevelUnderflow { row: 3, level: -5 })
        );
    }

    #[test]
    fn empty_and_underflowing_inputs_raise() {
        assert_eq!(NodeList::from_rows(rows(&[])), Err(TreeError::Empty));
        assert_eq!(
            NodeList::from_rows(rows(&[0, 1, -1])),
            Err(TreeError::LevelUnderflow { row: 2, level: -1 })
        );
        assert_eq!(
            NodeList::from_rows(rows(&[-2])),
            Err(TreeError::LevelUnderflow { row: 0, level: -2 })
        );
    }

    #[test]
    fn a_root_at_level_minus_one_is_kept() {
        let t = NodeList::from_rows(rows(&[-1, 0, 0])).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.children(t.root()), &[NodeId(1), NodeId(2)]);
    }

    #[test]
    fn siblings_and_deeper_subtrees() {
        // Base { a { b, c }, d }
        let t = NodeList::from_rows(rows(&[0, 1, 2, 2, 1])).unwrap();
        assert_eq!(t.children(NodeId(0)), &[NodeId(1), NodeId(4)]);
        assert_eq!(t.children(NodeId(1)), &[NodeId(2), NodeId(3)]);
        assert_eq!(t.children(NodeId(4)), []);
        assert_eq!(t.children(NodeId(99)), []);
        assert_eq!(t.name(NodeId(4)), b"n4");
    }
}
