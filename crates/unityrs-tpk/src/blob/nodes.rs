// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `TpkUnityNodeBuffer` (`helpers/Tpk.py:394-397`): every `TpkUnityNode`
//! (`:351-382`), the nodes `generate_node` walks.

use std::sync::Arc;

use unityrs_core::limits::{Meter, MeteredVec};

use super::{Stream, bounded, memory, range_count};
use crate::TpkError;

/// A `TpkUnityNode` (`Tpk.py:370-382`): `<HHihbIH`, then its sub-node ids.
/// Its type flags (the `b`) are read and not kept: `generate_node` does not
/// copy them (`:73-83`).
pub(crate) struct Node {
    /// `TypeName`, an id into the string buffer.
    pub(crate) type_name: u16,
    /// `Name`, an id into the string buffer.
    pub(crate) name: u16,
    /// `ByteSize`.
    pub(crate) byte_size: i32,
    /// `Version`.
    pub(crate) version: i16,
    /// `MetaFlag`, unsigned as stored (the `I`).
    pub(crate) meta_flag: u32,
    /// Where its `SubNodes` start in [`NodeBuffer`]'s one table of them.
    first: usize,
    /// How many it has (the `H` count).
    count: u16,
}

/// `NodeBuffer`: the nodes, and every node's `SubNodes` in one table, each
/// node's a run of it.
pub(crate) struct NodeBuffer {
    nodes: MeteredVec<Node>,
    sub_nodes: MeteredVec<u16>,
}

impl NodeBuffer {
    /// `TpkUnityNodeBuffer(stream)`: a count (a `range`: a negative one is
    /// none), then each node. A node's sub-node ids are read as
    /// `Struct(f"<{count}H")` reads them, all at once, before they are kept.
    pub(crate) fn read(s: &mut Stream<'_>, meter: &Arc<Meter>) -> Result<Self, TpkError> {
        let count = bounded("nodes", range_count(s.i32("the node count")?))?;
        let mut nodes = MeteredVec::new(meter);
        let mut sub_nodes = MeteredVec::new(meter);
        for _ in 0..count {
            let type_name = s.u16("a node's type name")?;
            let name = s.u16("a node's name")?;
            let byte_size = s.i32("a node's byte size")?;
            let version = s.i16("a node's version")?;
            s.u8("a node's type flags")?;
            let meta_flag = s.u32("a node's meta flag")?;
            let n = s.u16("a node's sub-node count")?;
            let (ids, _) = s
                .read(usize::from(n) * 2, "a node's sub-node ids")?
                .as_chunks::<2>();
            let first = sub_nodes.len();
            sub_nodes.reserve(ids.len()).map_err(memory)?;
            for id in ids {
                sub_nodes.push(u16::from_le_bytes(*id)).map_err(memory)?;
            }
            nodes
                .push(Node {
                    type_name,
                    name,
                    byte_size,
                    version,
                    meta_flag,
                    first,
                    count: n,
                })
                .map_err(memory)?;
        }
        Ok(Self { nodes, sub_nodes })
    }

    /// `NodeBuffer[node_id]`; `None` where UnityPy raises `IndexError`.
    pub(crate) fn get(&self, id: u16) -> Option<&Node> {
        self.nodes.as_slice().get(usize::from(id))
    }

    /// `node.SubNodes`, in their stored order.
    pub(crate) fn sub_nodes(&self, node: &Node) -> &[u16] {
        let end = node.first.saturating_add(usize::from(node.count));
        self.sub_nodes
            .as_slice()
            .get(node.first..end)
            .unwrap_or_default()
    }
}
