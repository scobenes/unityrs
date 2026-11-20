// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Preserve the node records used for object construction and `UnknownObject`.
//! Python owns returned nodes. All Rust-to-Python staging is charged, and the
//! reference index built, detached; one attachment then builds the objects.
use crate::{errors, python::held as h, value_render::Pace};
use pyo3::prelude::*;
use std::collections::HashMap;
use unityrs_core::{
    files::serialized::SerializedType,
    limits::Charge,
    typetree::{NodeId, SharedNodes},
};

pub(crate) fn convert(node_lists: &[SharedNodes], charge: &mut Charge) -> PyResult<Py<PyAny>> {
    charge.grow(64).map_err(errors::charge)?;
    for list in node_lists {
        charge.grow(64).map_err(errors::charge)?;
        for (index, row) in list.rows().iter().enumerate() {
            let id = NodeId(u32::try_from(index)?);
            let text = row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64;
            let children = list.children(id).len() as u64;
            charge
                .grow(4096 + text.saturating_mul(8))
                .map_err(errors::charge)?;
            charge
                .meter()
                .count_decode(
                    4096 + text
                        .saturating_mul(4)
                        .saturating_add(children.saturating_mul(64)),
                )
                .map_err(errors::charge)?;
            charge
                .grow(children.saturating_mul(64))
                .map_err(errors::charge)?;
        }
    }
    crate::python::relay::attach(|py| build_nodes(py, node_lists).map(Bound::unbind))
}

const FIELDS: [&str; 11] = [
    "m_Type",
    "m_Name",
    "m_Level",
    "m_ByteSize",
    "m_Version",
    "m_TypeFlags",
    "m_VariableCount",
    "m_Index",
    "m_MetaFlag",
    "m_RefTypeHash",
    "m_Children",
];

fn build_nodes<'py>(py: Python<'py>, node_lists: &[SharedNodes]) -> PyResult<Bound<'py, PyAny>> {
    let mut keys = Vec::new();
    keys.try_reserve_exact(FIELDS.len())
        .map_err(|_| pyo3::exceptions::PyMemoryError::new_err("node key allocation"))?;
    for name in FIELDS {
        keys.push(h::string(py, name.as_bytes())?);
    }
    let [
        ty,
        name,
        level,
        size,
        version,
        flags,
        count,
        index,
        meta,
        hash,
        children,
    ] = keys.as_slice()
    else {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("node keys"));
    };
    let optional = |value: Option<i64>| value.map_or_else(|| Ok(h::none(py)), |n| h::int(py, n));
    let mut pace = Pace::new();
    let lists = h::list(py)?;
    for list in node_lists {
        let rows = h::list(py)?;
        for (at, row) in list.rows().iter().enumerate() {
            pace.tick(py);
            let id = NodeId(u32::try_from(at)?);
            let out = h::dict(py)?;
            h::set(&out, ty, &h::string(py, row.ty.as_bytes())?)?;
            h::set(&out, name, &h::string(py, row.name.as_bytes())?)?;
            h::set(&out, level, &h::int(py, i64::from(row.level))?)?;
            h::set(&out, size, &h::int(py, i64::from(row.byte_size))?)?;
            h::set(&out, version, &h::int(py, i64::from(row.version))?)?;
            h::set(&out, flags, &optional(row.type_flags.map(i64::from))?)?;
            h::set(&out, count, &optional(row.variable_count.map(i64::from))?)?;
            h::set(&out, index, &optional(row.index.map(i64::from))?)?;
            h::set(&out, meta, &optional(row.meta_flag)?)?;
            let ref_hash = match row.ref_type_hash {
                Some(n) => h::uint(py, n)?,
                None => h::none(py),
            };
            h::set(&out, hash, &ref_hash)?;
            let kids = h::list(py)?;
            for child in list.children(id) {
                pace.tick(py);
                h::push(&kids, &h::uint(py, u64::from(child.0))?)?;
            }
            h::set(&out, children, &kids)?;
            h::push(&rows, &out)?;
        }
        h::push(&lists, &rows)?;
    }
    Ok(lists)
}

pub(crate) fn references(
    node_lists: &[SharedNodes],
    refs: Option<&[SerializedType]>,
    charge: &mut Charge,
) -> PyResult<Py<PyAny>> {
    charge.grow(64).map_err(errors::charge)?;
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
    let found = |reference: &SerializedType| {
        let nodes = reference.nodes.as_ref()?;
        indices
            .get(&(std::ptr::from_ref(&**nodes) as usize))
            .copied()
    };
    let mut records = Vec::new();
    for reference in refs {
        let Some(index) = found(reference) else {
            continue;
        };
        charge.grow(256).map_err(errors::charge)?;
        for name in names(reference) {
            charge
                .grow((name.len() as u64).saturating_mul(8))
                .map_err(errors::charge)?;
            charge
                .meter()
                .count_decode((name.len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
        }
        // Grown after the record's charge, which covers its slot.
        records
            .try_reserve(1)
            .map_err(|_| pyo3::exceptions::PyMemoryError::new_err("reference record allocation"))?;
        records.push((reference, index));
    }
    crate::python::relay::attach(|py| {
        let result = h::list(py)?;
        let mut pace = Pace::new();
        for (reference, index) in &records {
            pace.tick(py);
            let record = h::list(py)?;
            for name in names(reference) {
                h::push(&record, &h::string(py, name)?)?;
            }
            h::push(&record, &h::uint(py, *index as u64)?)?;
            h::push(&result, &record)?;
        }
        Ok(result.unbind())
    })
}

fn names(reference: &SerializedType) -> [&[u8]; 3] {
    [
        &reference.class_name,
        &reference.namespace,
        &reference.assembly,
    ]
    .map(|name| {
        name.as_ref()
            .map_or(&[][..], unityrs_core::RawStr::as_bytes)
    })
}
