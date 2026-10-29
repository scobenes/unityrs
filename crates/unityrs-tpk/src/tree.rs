// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `generate_node` (`helpers/Tpk.py:58-88`): a class's tree, walked from its
//! release root over the node buffer, as the rows UnityPy builds before
//! `TypeTreeNode.from_list`.

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::typetree::NodeRow;

use crate::blob::TypeTreeBlob;

/// Why a walk stopped.
#[derive(Debug)]
pub(crate) enum WalkError {
    /// A node at `depth` levels (the root's is 1), past the `max` the walk
    /// allows: a level of `max` is a depth of `max + 1`, as unityrs-core's
    /// `Tree::compile` counts.
    Depth { depth: usize, max: usize },
    /// The rows walked and still to walk, past the `max` the walk allows.
    Rows { rows: usize, max: usize },
    /// A node or string id past its table, where UnityPy raises
    /// `IndexError`.
    PastTable(&'static str),
    /// A growth the allocator refused.
    Memory(ChargeError),
}

impl From<ChargeError> for WalkError {
    fn from(e: ChargeError) -> Self {
        Self::Memory(e)
    }
}

/// `generate_node(class_info)` from `root`, its release root (`Tpk.py:66-85`):
/// every node in pre-order (`stack.pop(0)`, its sub-nodes put in front, in
/// their order), each row with its level (its parent's plus one), its
/// position as `m_Index`, its byte size, version and meta flag (unsigned, as
/// stored), its type and name from the string buffer, and its type flags,
/// variable count and ref-type hash unset (`TypeTreeNode`'s keywords there).
///
/// UnityPy's walk has no bound; this one stops at a level of `max_depth` or
/// past `max_rows` rows, counting those still to walk, since each becomes a
/// row (crate docs, "Limits"). So a node that lists itself or an ancestor,
/// on which UnityPy's walk never ends, is an error. Every row and the
/// pending stack are charged to `meter`.
pub(crate) fn generate_node(
    blob: &TypeTreeBlob,
    root: u16,
    max_depth: usize,
    max_rows: usize,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<NodeRow>, WalkError> {
    // A level is an `i32` (`NodeRow::level`); every one is below this.
    let max_depth = max_depth.min(usize::try_from(i32::MAX).unwrap_or(usize::MAX));
    let string = |id: u16| {
        blob.strings
            .as_slice()
            .get(usize::from(id))
            .cloned()
            .ok_or(WalkError::PastTable(
                "IndexError: a string id past the string buffer",
            ))
    };
    let mut nodes = MeteredVec::new(meter);
    // `stack = [(class_info.ReleaseRootNode, 0)]`, its front at the end.
    let mut stack = MeteredVec::new(meter);
    stack.push((root, 0_usize))?;
    while let Some(&(node_id, level)) = stack.as_slice().last() {
        // `node_id, level = stack.pop(0)`.
        stack.truncate(stack.len().saturating_sub(1));
        // `node: TpkUnityNode = NODES[node_id]`.
        let node = blob.nodes.get(node_id).ok_or(WalkError::PastTable(
            "IndexError: a node id past the node buffer",
        ))?;
        let row = NodeRow {
            // Below `max_depth`, which is at most `i32::MAX`.
            level: i32::try_from(level).unwrap_or(i32::MAX),
            ty: string(node.type_name)?,
            name: string(node.name)?,
            byte_size: node.byte_size,
            version: i32::from(node.version),
            type_flags: None,
            variable_count: None,
            // `m_Index=index`: below `max_rows`.
            index: Some(i32::try_from(nodes.len()).unwrap_or(i32::MAX)),
            // UnityPy keeps the stored `u32` (`Tpk.py:361`), zero-extended.
            meta_flag: Some(i64::from(node.meta_flag)),
            ref_type_hash: None,
        };
        nodes.push(row)?;
        let subs = blob.nodes.sub_nodes(node);
        if subs.is_empty() {
            continue;
        }
        // Each bound is its own error. A sub-node's depth, counting the
        // root as 1, is its level plus one.
        if level + 1 >= max_depth {
            return Err(WalkError::Depth {
                depth: level + 2,
                max: max_depth,
            });
        }
        let held = nodes.len() + stack.len() + subs.len();
        if held > max_rows {
            return Err(WalkError::Rows {
                rows: held,
                max: max_rows,
            });
        }
        // `stack = [(node_id, level + 1) for node_id in node.SubNodes] + stack`.
        for &id in subs.iter().rev() {
            stack.push((id, level + 1))?;
        }
    }
    Ok(nodes)
}
