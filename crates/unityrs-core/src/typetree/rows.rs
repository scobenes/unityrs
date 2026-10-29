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
//! Owner: the scaffold (implemented). Package B compiles a `NodeList` into a
//! [`super::Tree`]; package C produces rows from files.

use crate::error::LimitError;
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
    pub meta_flag: Option<i32>,
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
    pub fn with_meta_flag(mut self, meta_flag: i32) -> Self {
        self.meta_flag = Some(meta_flag);
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
    /// A compiled-tree bound (depth) was exceeded.
    #[error(transparent)]
    Limit(#[from] LimitError),
}

/// The kept tree of a typetree, as pre-order rows with their child lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeList {
    rows: Box<[NodeRow]>,
    /// `child_start[i]..child_start[i + 1]` indexes `children` for node `i`.
    child_start: Box<[u32]>,
    children: Box<[NodeId]>,
}

fn to_u32(i: usize) -> u32 {
    // Callers have checked the row count against `u32::MAX`.
    u32::try_from(i).unwrap_or(u32::MAX)
}

impl NodeList {
    /// Builds the tree by UnityPy's rule (module docs). The input is every row
    /// the file stores, in order.
    pub fn from_rows(mut rows: Vec<NodeRow>) -> Result<Self, TreeError> {
        if rows.is_empty() {
            return Err(TreeError::Empty);
        }
        if u32::try_from(rows.len()).is_err() {
            return Err(TreeError::TooManyNodes(rows.len()));
        }
        // `None` is the hidden root, at level -1.
        let level_of = |rows: &[NodeRow], node: Option<usize>| -> i64 {
            node.and_then(|i| rows.get(i))
                .map_or(-1, |r| i64::from(r.level))
        };
        let mut stack: Vec<Option<usize>> = vec![None];
        let mut parent: Option<usize> = None;
        let mut prev: Option<usize> = None;
        let mut parents: Vec<Option<usize>> = Vec::with_capacity(rows.len());
        let mut kept_end: Option<usize> = None;
        for (i, row) in rows.iter().enumerate() {
            let level = i64::from(row.level);
            let prev_level = level_of(&rows, prev);
            if level > prev_level {
                stack.push(parent);
                parent = prev;
            } else if level < prev_level {
                while level <= level_of(&rows, parent) {
                    parent = stack.pop().ok_or(TreeError::LevelUnderflow {
                        row: i,
                        level: row.level,
                    })?;
                }
            }
            if parent.is_none() && i > 0 && kept_end.is_none() {
                kept_end = Some(i);
            }
            if kept_end.is_none() {
                parents.push(parent);
            }
            prev = Some(i);
        }
        rows.truncate(kept_end.unwrap_or(rows.len()));

        let mut lists: Vec<Vec<NodeId>> = vec![Vec::new(); rows.len()];
        for (i, p) in parents.iter().enumerate().skip(1) {
            if let Some(list) = p.and_then(|p| lists.get_mut(p)) {
                list.push(NodeId(to_u32(i)));
            }
        }
        let mut child_start = Vec::with_capacity(rows.len() + 1);
        let mut children = Vec::with_capacity(rows.len().saturating_sub(1));
        for list in lists {
            child_start.push(to_u32(children.len()));
            children.extend(list);
        }
        child_start.push(to_u32(children.len()));
        Ok(Self {
            rows: rows.into_boxed_slice(),
            child_start: child_start.into_boxed_slice(),
            children: children.into_boxed_slice(),
        })
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
    use super::{NodeId, NodeList, NodeRow, TreeError};

    fn rows(levels: &[i32]) -> Vec<NodeRow> {
        levels
            .iter()
            .enumerate()
            .map(|(i, l)| NodeRow::new(*l, "int", format!("n{i}").as_str()))
            .collect()
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
        assert_eq!(NodeList::from_rows(Vec::new()), Err(TreeError::Empty));
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
