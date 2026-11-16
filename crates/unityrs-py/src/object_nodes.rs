// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Preserve the node records used for object construction and `UnknownObject`.
//! Python owns returned nodes; all Rust-to-Python staging is charged first.
use crate::{errors, python as p};
use pyo3::prelude::*;
use std::collections::HashMap;
use unityrs_core::{limits::Charge, typetree::NodeId};

pub(crate) fn convert(
    node_lists: &[unityrs_core::typetree::SharedNodes],
    charge: &mut Charge,
) -> PyResult<Py<PyAny>> {
    charge.grow(64).map_err(errors::charge)?;
    let lists = p::list()?;
    for list in node_lists {
        charge.grow(64).map_err(errors::charge)?;
        let rows = p::list()?;
        for (index, row) in list.rows().iter().enumerate() {
            let id = NodeId(u32::try_from(index)?);
            charge
                .grow(
                    4096 + (row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64)
                        .saturating_mul(8),
                )
                .map_err(errors::charge)?;
            charge
                .meter()
                .count_decode(
                    4096 + (row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64)
                        .saturating_mul(4)
                        .saturating_add((list.children(id).len() as u64).saturating_mul(64)),
                )
                .map_err(errors::charge)?;
            let out = p::dict()?;
            p::field(&out, "m_Type", &p::string(row.ty.as_bytes())?)?;
            p::field(&out, "m_Name", &p::string(row.name.as_bytes())?)?;
            for (key, val) in [
                ("m_Level", Some(i64::from(row.level))),
                ("m_ByteSize", Some(i64::from(row.byte_size))),
                ("m_Version", Some(i64::from(row.version))),
                ("m_TypeFlags", row.type_flags.map(i64::from)),
                ("m_VariableCount", row.variable_count.map(i64::from)),
                ("m_Index", row.index.map(i64::from)),
                ("m_MetaFlag", row.meta_flag),
            ] {
                p::field(
                    &out,
                    key,
                    &match val {
                        Some(n) => p::int(n)?,
                        None => p::none(),
                    },
                )?;
            }
            p::field(
                &out,
                "m_RefTypeHash",
                &match row.ref_type_hash {
                    Some(n) => p::uint(n)?,
                    None => p::none(),
                },
            )?;
            let children = p::list()?;
            charge
                .grow((list.children(id).len() as u64).saturating_mul(64))
                .map_err(errors::charge)?;
            for child in list.children(id) {
                p::push(&children, &p::uint(u64::from(child.0))?)?;
            }
            p::field(&out, "m_Children", &children)?;
            p::push(&rows, &out)?;
        }
        p::push(&lists, &rows)?;
    }
    Ok(lists)
}

pub(crate) fn references(
    node_lists: &[unityrs_core::typetree::SharedNodes],
    refs: Option<&[unityrs_core::files::serialized::SerializedType]>,
    charge: &mut Charge,
) -> PyResult<Py<PyAny>> {
    charge.grow(64).map_err(errors::charge)?;
    let result = p::list()?;
    let refs = refs.unwrap_or_default();
    // Index the used lists once: scanning them for every reference costs
    // refs * lists even when no reference ultimately contributes a record.
    charge
        .grow(64 + (node_lists.len() as u64).saturating_mul(128))
        .map_err(errors::charge)?;
    charge
        .meter()
        .count_decode((refs.len() as u64 + node_lists.len() as u64).saturating_mul(64))
        .map_err(errors::charge)?;
    let mut indices = HashMap::new();
    indices
        .try_reserve(node_lists.len())
        .map_err(|_| pyo3::exceptions::PyMemoryError::new_err("reference list index allocation"))?;
    for (index, list) in node_lists.iter().enumerate() {
        indices.insert(std::ptr::from_ref(&**list) as usize, index);
    }
    for reference in refs {
        let Some(nodes) = &reference.nodes else {
            continue;
        };
        let Some(index) = indices.get(&(std::ptr::from_ref(&**nodes) as usize)) else {
            continue;
        };
        charge.grow(256).map_err(errors::charge)?;
        let record = p::list()?;
        for name in [
            &reference.class_name,
            &reference.namespace,
            &reference.assembly,
        ] {
            let bytes = name
                .as_ref()
                .map_or(&[][..], unityrs_core::RawStr::as_bytes);
            charge
                .grow((bytes.len() as u64).saturating_mul(8))
                .map_err(errors::charge)?;
            charge
                .meter()
                .count_decode((bytes.len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::push(&record, &p::string(bytes)?)?;
        }
        p::push(&record, &p::uint(*index as u64)?)?;
        p::push(&result, &record)?;
    }
    Ok(result)
}
