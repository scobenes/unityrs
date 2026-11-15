// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! `from_list` dictionaries and mutable node lists (`helpers/TypeTreeNode.py:156-188`).
//! The arena keeps node identity: relinking appends children in place, including
//! duplicate edges. A compiled snapshot can be supplied as an object's `nodes=`.

use std::sync::Arc;

use super::{NodeList, NodeRow, TreeError};
use crate::limits::{Charge, ChargeError, Limits, Meter, MeteredVec, heap_cost};
use crate::raw::RawStr;

/// A node dictionary before the first dictionary supplies the defaults.
/// Absent required keys remain distinct from an explicit zero or empty name.
#[derive(Clone, Debug, Default)]
pub struct NodeDict {
    /// `m_Level`.
    pub level: Option<i32>,
    /// `m_Type`.
    pub ty: Option<RawStr>,
    /// `m_Name`.
    pub name: Option<RawStr>,
    /// `m_ByteSize`.
    pub byte_size: Option<i32>,
    /// `m_Version`.
    pub version: Option<i32>,
    /// Constructor-supplied `m_Children` instances, retained before linked
    /// children. Their current edges are copied into this arena; reusing an
    /// instance, including indirectly, shares one local identity. Relinking
    /// mutates this arena's edges.
    pub children: Vec<Arc<TypeTreeNode>>,
    /// Optional node fields, copied without defaults.
    pub fields: NodeRow,
}

/// Dictionary construction errors retain the Python exception category.
#[derive(Debug, thiserror::Error)]
pub enum FromListError {
    /// First dictionary lacks a required key.
    #[error("Nodes must contain at least m_Level, m_Type and m_Name")]
    Required,
    /// A required constructor argument absent after applying the defaults.
    #[error("function missing required argument '{name}' (pos {position})")]
    MissingArgument {
        /// Python keyword.
        name: &'static str,
        /// One-based constructor position.
        position: usize,
    },
    /// A keyword repeated by the first dictionary's default patch.
    #[error(
        "UnityPy.helpers.TypeTreeNode.TypeTreeNode() got multiple values for keyword argument '{0}'"
    )]
    DuplicateArgument(&'static str),
    /// The tree builder or its memory allowance refused the input.
    #[error("{}", tree_message(.0))]
    Tree(#[from] TreeError),
}

fn tree_message(error: &TreeError) -> &dyn std::fmt::Display {
    match error {
        TreeError::Empty => &"list index out of range",
        TreeError::LevelUnderflow { .. } => &"pop from empty list",
        _ => error,
    }
}

impl FromListError {
    /// Exception to raise at a dynamic-language boundary.
    #[must_use]
    pub fn python_kind(&self) -> &'static str {
        match self {
            Self::Required | Self::Tree(TreeError::Cycle(_)) => "ValueError",
            Self::MissingArgument { .. } | Self::DuplicateArgument(_) => "TypeError",
            Self::Tree(
                TreeError::Empty | TreeError::LevelUnderflow { .. } | TreeError::UnknownNode(_),
            ) => "IndexError",
            Self::Tree(_) => "MemoryError",
        }
    }
}

impl From<ChargeError> for FromListError {
    fn from(e: ChargeError) -> Self {
        Self::Tree(e.into())
    }
}

/// Mutable node identities and their children. All growth is charged and fallible.
#[derive(Debug)]
pub struct TypeTreeNode {
    rows: MeteredVec<NodeRow>,
    children: MeteredVec<MeteredVec<usize>>,
    identities: MeteredVec<NodeIdentity>,
    // Keep supplied instances alive (and shared, hence not mutably borrowable)
    // for as long as this arena refers to their identities.
    sources: MeteredVec<Arc<Self>>,
    root: usize,
}

#[derive(Debug)]
struct IdentityOwner {
    _charge: Charge,
}

#[derive(Clone, Debug)]
struct NodeIdentity {
    owner: triomphe::Arc<IdentityOwner>,
    node: usize,
}

const _: () = {
    crate::send_sync::<NodeDict>();
    crate::send_sync::<FromListError>();
    crate::send_sync::<TypeTreeNode>();
};

impl TypeTreeNode {
    /// Construct one node directly, retaining supplied children without applying
    /// the level-linking rule. Unlike `from_list`, any stored level is accepted.
    pub fn new(
        row: &NodeRow,
        children: &[Arc<Self>],
        meter: &Arc<Meter>,
    ) -> Result<Self, TreeError> {
        crate::limits::TreeTraversal::new(meter, u32::MAX).step(
            0,
            0,
            1_u64
                .saturating_add(u64::try_from(row.ty.len()).unwrap_or(u64::MAX))
                .saturating_add(u64::try_from(row.name.len()).unwrap_or(u64::MAX)),
        )?;
        let mut rows = MeteredVec::with_capacity(meter, 1)?;
        rows.push(row.copy_charged(meter)?)?;
        let mut this = Self::unlinked(rows)?;
        this.import_children(std::iter::once((0, children)))?;
        Ok(this)
    }

    fn unlinked(rows: MeteredVec<NodeRow>) -> Result<Self, TreeError> {
        if rows.is_empty() {
            return Err(TreeError::Empty);
        }
        let meter = rows.charge().meter().clone();
        let bytes =
            heap_cost(u64::try_from(std::mem::size_of::<IdentityOwner>() + 16).unwrap_or(u64::MAX));
        let owner = triomphe::Arc::try_new(IdentityOwner {
            _charge: meter.charge(bytes)?,
        })
        .map_err(|_| meter.allocator_refused(bytes))?;
        let mut children = MeteredVec::with_capacity(&meter, rows.len())?;
        let mut identities = MeteredVec::with_capacity(&meter, rows.len())?;
        for node in 0..rows.len() {
            meter.count_decode(1)?;
            children.push(MeteredVec::new(&meter))?;
            identities.push(NodeIdentity {
                owner: owner.clone(),
                node,
            })?;
        }
        Ok(Self {
            rows,
            children,
            identities,
            sources: MeteredVec::new(&meter),
            root: 0,
        })
    }

    fn import_children<'a>(
        &mut self,
        supplied: impl Iterator<Item = (usize, &'a [Arc<Self>])>,
    ) -> Result<(), TreeError> {
        let meter = self.rows.charge().meter().clone();
        let mut imported = MeteredVec::new(&meter);
        let mut pending = MeteredVec::new(&meter);
        for (parent, children) in supplied {
            meter.count_decode(1)?;
            for child in children.iter().rev() {
                meter.count_decode(1)?;
                self.sources.push(child.clone())?;
                pending.push((child.as_ref(), child.root, parent))?;
            }
            while let Some((source, id, parent)) = pending.pop() {
                meter.count_decode(1)?;
                let identity = source
                    .identities
                    .as_slice()
                    .get(id)
                    .ok_or(TreeError::UnknownNode(id))?;
                let mut found = None;
                for &(key, target) in imported.as_slice() {
                    let key: &NodeIdentity = key;
                    meter.count_decode(1)?;
                    if triomphe::Arc::ptr_eq(&key.owner, &identity.owner)
                        && key.node == identity.node
                    {
                        found = Some(target);
                        break;
                    }
                }
                let target = if let Some(target) = found {
                    target
                } else {
                    let row = source
                        .rows
                        .as_slice()
                        .get(id)
                        .ok_or(TreeError::UnknownNode(id))?;
                    meter.count_decode(
                        u64::try_from(row.ty.len())
                            .unwrap_or(u64::MAX)
                            .saturating_add(u64::try_from(row.name.len()).unwrap_or(u64::MAX)),
                    )?;
                    let target = self.rows.len();
                    self.rows.push(row.copy_charged(&meter)?)?;
                    self.children.push(MeteredVec::new(&meter))?;
                    self.identities.push(identity.clone())?;
                    imported.push((identity, target))?;
                    let children = source
                        .children
                        .as_slice()
                        .get(id)
                        .ok_or(TreeError::UnknownNode(id))?;
                    for &child in children.as_slice().iter().rev() {
                        meter.count_decode(1)?;
                        pending.push((source, child, target))?;
                    }
                    target
                };
                self.children
                    .as_mut_slice()
                    .get_mut(parent)
                    .ok_or(TreeError::UnknownNode(parent))?
                    .push(target)?;
            }
        }
        Ok(())
    }

    /// Construct dictionaries with the defaults decided by the first alone.
    /// Existing nodes use [`Self::from_nodes`] and [`Self::relink`].
    pub fn from_list(dicts: &[NodeDict], meter: &Arc<Meter>) -> Result<Self, FromListError> {
        let mut bounds = crate::limits::TreeTraversal::new(meter, u32::MAX);
        let first = dicts.first().ok_or(TreeError::Empty)?;
        if first.level.is_none() || first.ty.is_none() || first.name.is_none() {
            return Err(FromListError::Required);
        }
        let mut rows = MeteredVec::with_capacity(meter, dicts.len())?;
        for d in dicts {
            bounds.step(0, 0, 1)?;
            // Python merges **kwargs before invoking the constructor: duplicate
            // defaults take precedence over missing required arguments.
            for (name, defaulted, supplied) in [
                (
                    "m_ByteSize",
                    first.byte_size.is_none(),
                    d.byte_size.is_some(),
                ),
                ("m_Version", first.version.is_none(), d.version.is_some()),
            ] {
                if defaulted && supplied {
                    return Err(FromListError::DuplicateArgument(name));
                }
            }
            let missing = |name, position| FromListError::MissingArgument { name, position };
            let mut row = NodeRow {
                type_flags: d.fields.type_flags,
                variable_count: d.fields.variable_count,
                index: d.fields.index,
                meta_flag: d.fields.meta_flag,
                ref_type_hash: d.fields.ref_type_hash,
                ..NodeRow::default()
            };
            row.level = d.level.ok_or_else(|| missing("m_Level", 1))?;
            let ty = d.ty.as_ref().ok_or_else(|| missing("m_Type", 2))?;
            let name = d.name.as_ref().ok_or_else(|| missing("m_Name", 3))?;
            meter.count_decode(
                u64::try_from(ty.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(name.len()).unwrap_or(u64::MAX)),
            )?;
            row.ty = RawStr::charged(ty.as_bytes(), meter)?;
            row.name = RawStr::charged(name.as_bytes(), meter)?;
            let field =
                |first: Option<i32>, later: Option<i32>, name, position| match (first, later) {
                    (None, None) => Ok(0),
                    (Some(_), Some(v)) => Ok(v),
                    _ => Err(missing(name, position)),
                };
            row.byte_size = field(first.byte_size, d.byte_size, "m_ByteSize", 4)?;
            row.version = field(first.version, d.version, "m_Version", 5)?;
            rows.push(row)?;
        }
        let mut ids = MeteredVec::with_capacity(meter, dicts.len())?;
        for i in 0..dicts.len() {
            meter.count_decode(1)?;
            ids.push(i)?;
        }
        let mut this = Self::unlinked(rows)?;
        this.import_children(
            dicts
                .iter()
                .enumerate()
                .map(|(i, d)| (i, d.children.as_slice())),
        )?;
        this.relink_nodes(ids.as_slice())?;
        Ok(this)
    }

    /// Build an initially childless list of node instances. Calling `relink`
    /// again models `from_list(list(node.traverse()))` for a single-root list.
    pub fn from_nodes(rows: MeteredVec<NodeRow>) -> Result<Self, TreeError> {
        let meter = rows.charge().meter().clone();
        let mut ids = MeteredVec::with_capacity(&meter, rows.len())?;
        for i in 0..rows.len() {
            meter.count_decode(1)?;
            ids.push(i)?;
        }
        let mut this = Self::unlinked(rows)?;
        this.relink_nodes(ids.as_slice())?;
        Ok(this)
    }

    /// `from_list(list(root.traverse()))`: snapshot the current traversal,
    /// including every duplicate identity, then append the child edges in place.
    pub fn relink(&mut self) -> Result<(), TreeError> {
        let ids = self.traversal(&Limits::default())?;
        self.relink_nodes(ids.as_slice())
    }

    /// Link an explicit list of existing arena identities, in the given order.
    /// The list may repeat or omit nodes; existing children remain attached.
    /// Like UnityPy, a later invalid level can fail after earlier mutations.
    pub fn relink_nodes(&mut self, ids: &[usize]) -> Result<(), TreeError> {
        if ids.is_empty() {
            return Err(TreeError::Empty);
        }
        let mut stack = MeteredVec::new(self.rows.charge().meter());
        let hidden = usize::MAX;
        let level = |n: usize| {
            self.rows
                .as_slice()
                .get(n)
                .map_or(-1, |r| i64::from(r.level))
        };
        stack.push(hidden)?;
        let (mut parent, mut prev) = (hidden, hidden);
        let mut root = None;
        for (position, &i) in ids.iter().enumerate() {
            self.rows.charge().meter().count_decode(1)?;
            let row = self
                .rows
                .as_slice()
                .get(i)
                .ok_or(TreeError::UnknownNode(i))?;
            let l = i64::from(row.level);
            if l > level(prev) {
                stack.push(parent)?;
                parent = prev;
            } else if l < level(prev) {
                while l <= level(parent) {
                    self.rows.charge().meter().count_decode(1)?;
                    parent = stack.pop().ok_or(TreeError::LevelUnderflow {
                        row: position,
                        level: row.level,
                    })?;
                }
            }
            if parent == hidden {
                root.get_or_insert(i);
            } else if let Some(children) = self.children.as_mut_slice().get_mut(parent) {
                children.push(i)?;
            }
            prev = i;
        }
        self.root = root.ok_or(TreeError::Empty)?;
        Ok(())
    }

    /// Visit one edge at a time with a charged path and active-identity table.
    /// Repeated DAG edges are visited independently; cycles never expand.
    fn visit(
        &self,
        limits: &Limits,
        mut output: impl FnMut(usize, u32) -> Result<u32, TreeError>,
    ) -> Result<(), TreeError> {
        let meter = self.rows.charge().meter();
        let mut bounds = crate::limits::TreeTraversal::new(meter, limits.max_tree_depth);
        let mut active = MeteredVec::with_capacity(meter, self.rows.len())?;
        for _ in 0..self.rows.len() {
            bounds.step(0, 0, 1)?;
            active.push(false)?;
        }
        // (identity, parent's output index, next edge, own output index).
        let mut stack = MeteredVec::new(meter);
        stack.push((self.root, u32::MAX, None::<usize>, 0u32))?;
        while let Some((id, parent, next, index)) = stack.as_slice().last().copied() {
            let depth = u64::try_from(stack.len()).unwrap_or(u64::MAX);
            bounds.step(depth, 0, 1)?;
            let children = self
                .children
                .as_slice()
                .get(id)
                .ok_or(TreeError::UnknownNode(id))?;
            if let Some(next) = next {
                if let Some(&child) = children.as_slice().get(next) {
                    bounds.step(depth.saturating_add(1), 0, 1)?;
                    if let Some(frame) = stack.as_mut_slice().last_mut() {
                        frame.2 = Some(next + 1);
                    }
                    stack.push((child, index, None, 0))?;
                } else {
                    if let Some(mark) = active.as_mut_slice().get_mut(id) {
                        *mark = false;
                    }
                    stack.pop();
                }
            } else {
                let mark = active
                    .as_mut_slice()
                    .get_mut(id)
                    .ok_or(TreeError::UnknownNode(id))?;
                if *mark {
                    return Err(TreeError::Cycle(id));
                }
                *mark = true;
                let index = output(id, parent)?;
                if let Some(frame) = stack.as_mut_slice().last_mut() {
                    frame.2 = Some(0);
                    frame.3 = index;
                }
            }
        }
        Ok(())
    }

    /// Current preorder identities, including repeated edges, with bounded
    /// depth, work and memory. Suitable for [`Self::relink_nodes`].
    pub fn traversal(&self, limits: &Limits) -> Result<MeteredVec<usize>, TreeError> {
        let mut rows = MeteredVec::new(self.rows.charge().meter());
        self.visit(limits, |id, _| {
            rows.push(id)?;
            Ok(0)
        })?;
        Ok(rows)
    }

    /// Snapshot the reachable rows for compilation and `nodes=` overrides.
    pub fn node_list(&self, limits: &Limits) -> Result<NodeList, TreeError> {
        let meter = self.rows.charge().meter();
        let mut rows = MeteredVec::new(meter);
        let mut parents = MeteredVec::new(meter);
        self.visit(limits, |i, parent| {
            let index =
                u32::try_from(rows.len()).map_err(|_| TreeError::TooManyNodes(rows.len()))?;
            let row = self
                .rows
                .as_slice()
                .get(i)
                .ok_or(TreeError::UnknownNode(i))?;
            meter.count_decode(
                u64::try_from(row.ty.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(row.name.len()).unwrap_or(u64::MAX)),
            )?;
            rows.push(row.copy_charged(meter)?)?;
            parents.push(parent)?;
            Ok(index)
        })?;
        // Count the linear adjacency-table passes before building them.
        meter.count_decode(
            u64::try_from(rows.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(6),
        )?;
        NodeList::from_preorder(rows, parents)
    }
}
