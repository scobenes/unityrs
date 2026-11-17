//! Read the compiled tables as discovery metadata. Only leaf-to-slot indexing
//! is interpreted. The retained dict supplies the remaining serialized facts.
use crate::{
    AnimError,
    read::{Dict, version},
};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    limits::{Meter, MeteredVec},
    value::{Parsed, PrimArray, Value},
};

/// Location of a leaf in the stored machine, state, tree and node arrays.
#[derive(Debug)]
pub struct ControllerLeaf {
    pub location: [usize; 4],
    /// Index in this controller's original `m_AnimationClips` array.
    pub slot: Result<usize, AnimError>,
}

/// The base controller and its original, charged typetree result.
#[derive(Debug)]
pub struct ControllerTable {
    pub object: ObjectRef,
    pub serialized: Parsed,
    pub(crate) leaves: MeteredVec<ControllerLeaf>,
}

impl ControllerTable {
    #[must_use]
    pub fn leaves(&self) -> &[ControllerLeaf] {
        self.leaves.as_slice()
    }

    /// Read TOS strings as raw evidence in the controller's domain.
    /// Every matching row is preserved, including duplicate hashes.
    pub fn strings(&self, hash: u32, meter: &Arc<Meter>) -> Result<MeteredVec<&[u8]>, AnimError> {
        let mut out = MeteredVec::new(meter);
        let dict = Dict {
            parsed: &self.serialized,
            meter,
        };
        if let Some(tos) = dict.root("m_TOS")? {
            for entry in dict.list(tos, "m_TOS")? {
                let Value::Tuple(pair) = entry else {
                    return Err(AnimError::Malformed("m_TOS pair"));
                };
                let [key, string] = pair.as_ref() else {
                    return Err(AnimError::Malformed("m_TOS pair"));
                };
                if key.as_u64() == Some(u64::from(hash)) {
                    out.push(
                        string
                            .as_bytes()
                            .ok_or(AnimError::Malformed("m_TOS string"))?,
                    )?;
                }
            }
        }
        Ok(out)
    }
}

pub(crate) fn read_leaves(
    object: &ObjectRef,
    dict: &Dict<'_, '_>,
    clip_count: usize,
) -> Result<MeteredVec<ControllerLeaf>, AnimError> {
    let mut leaves = MeteredVec::new(dict.meter);
    let Some(constant) = dict.root("m_Controller")? else {
        return Ok(leaves);
    };
    let machines = array(dict, constant, "m_StateMachineArray")?;
    let version = version(object);
    for (m, machine) in machines.iter().enumerate() {
        let machine = dict.required(machine, "data")?;
        for (s, state) in array(dict, machine, "m_StateConstantArray")?
            .iter()
            .enumerate()
        {
            let state = dict.required(state, "data")?;
            let info = dict.field(state, "m_LeafInfoArray")?;
            for (t, tree) in array(dict, state, "m_BlendTreeConstantArray")?
                .iter()
                .enumerate()
            {
                let tree = dict.required(tree, "data")?;
                for (n, node) in array(dict, tree, "m_NodeArray")?.iter().enumerate() {
                    let node = dict.required(node, "data")?;
                    let children = dict.required(node, "m_ChildIndices")?;
                    let Value::Prims(PrimArray::U32(children)) = children else {
                        return Err(AnimError::Malformed("m_ChildIndices"));
                    };
                    if !children.is_empty() {
                        continue;
                    }
                    let raw = uint(dict, node, "m_ClipID")?;
                    if raw != u32::MAX {
                        let slot = match leaf_slot(dict, raw, version, info, clip_count) {
                            Err(cause) if cause.resource_refusal() => return Err(cause),
                            slot => slot,
                        };
                        leaves.push(ControllerLeaf {
                            location: [m, s, t, n],
                            slot,
                        })?;
                    }
                }
            }
        }
    }
    Ok(leaves)
}

fn array<'a>(
    dict: &Dict<'a, '_>,
    parent: &'a Value,
    name: &'static str,
) -> Result<&'a [Value], AnimError> {
    dict.list(dict.required(parent, name)?, name)
}

fn uint(dict: &Dict<'_, '_>, row: &Value, field: &'static str) -> Result<u32, AnimError> {
    dict.required(row, field)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(AnimError::Malformed(field))
}

fn leaf_slot(
    dict: &Dict<'_, '_>,
    id: u32,
    version: Option<(u16, u16)>,
    info: Option<&Value>,
    clip_count: usize,
) -> Result<usize, AnimError> {
    // AssetStudio v0.16.47's AnimatorController reader documents the
    // pre-5.2 LeafInfoConstant fields. The state-local ID is relative to
    // the entry's index offset, while later m_ClipID values index directly.
    let slot = match (version, info) {
        (Some((4, _) | (5, 0..=1)), Some(info)) => legacy_leaf(dict, info, id)?,
        (Some(v), None) if v >= (5, 2) => id,
        _ => return Err(AnimError::UnsupportedControllerLayout),
    };
    usize::try_from(slot)
        .ok()
        .filter(|&slot| slot < clip_count)
        .ok_or(AnimError::InvalidClipSlot(slot))
}

fn legacy_leaf(dict: &Dict<'_, '_>, info: &Value, id: u32) -> Result<u32, AnimError> {
    let mut found = None;
    for row in dict.list(info, "m_LeafInfoArray")? {
        let Value::Prims(PrimArray::U32(ids)) = dict.required(row, "m_IDArray")? else {
            return Err(AnimError::Malformed("m_IDArray"));
        };
        dict.meter.count_decode(ids.len() as u64)?;
        let base = uint(dict, row, "m_IndexOffset")?;
        for (offset, _) in ids
            .iter()
            .enumerate()
            .filter(|(_, candidate)| **candidate == id)
        {
            let slot = u32::try_from(offset)
                .ok()
                .and_then(|i| base.checked_add(i))
                .ok_or(AnimError::UnmappedControllerLeaf(id))?;
            if found.replace(slot).is_some() {
                return Err(AnimError::UnmappedControllerLeaf(id));
            }
        }
    }
    found.ok_or(AnimError::UnmappedControllerLeaf(id))
}

const _: () = crate::send_sync::<ControllerLeaf>();
const _: () = crate::send_sync::<ControllerTable>();
