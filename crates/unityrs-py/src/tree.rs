// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Explicit node snapshots and object trees (helpers/TypeTreeNode.py:156-188,
//! files/ObjectReader.py:285-307). Python owns mutable identities; a read
//! compiles a fresh, charged snapshot of their current edges.
use crate::{errors, python as p, traversal::TreeTraversal};
use pyo3::{
    ffi,
    prelude::*,
    pybacked::PyBackedBytes,
    types::{PyByteArray, PyBytes, PyString},
};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, RawStr,
    limits::MeteredVec,
    typetree::{NodeList, NodeRow, Tree, TreeError},
};

pub(crate) fn tree_error(error: TreeError) -> PyErr {
    match error {
        TreeError::Charge(e) => errors::charge(e),
        TreeError::Limit(e) => errors::core(e.into()),
        _ => pyo3::exceptions::PyValueError::new_err(errors::message(&error)),
    }
}

pub(crate) fn object_nodes(object: &ObjectRef) -> PyResult<(Py<PyAny>, TreeTraversal)> {
    let found = crate::worker::run(object.file().context().limits.stack_needed(), || {
        object.tree(None)
    })?
    .map_err(errors::parse)?;
    let Some(tree) = found else {
        return Err(errors::parse(unityrs_core::walk::ParseError::NoTypeTree {
            class_id: object.class_id(),
            why: object.why_no_tree(),
        }));
    };
    let mut traversal = TreeTraversal::on(
        object.file().meter(),
        object.file().context().limits.max_tree_depth,
    );
    traversal.expand(u64::from(tree.tree.depth()), 0, 0)?;
    let rows = rows_to_python(tree.tree.list(), &mut traversal)?;
    Ok((rows, traversal))
}

pub(crate) fn rows_to_python(
    list: &unityrs_core::typetree::NodeList,
    traversal: &mut TreeTraversal,
) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| py.detach(|| rows_to_python_detached(list, traversal)))
}

fn rows_to_python_detached(
    list: &unityrs_core::typetree::NodeList,
    traversal: &mut TreeTraversal,
) -> PyResult<Py<PyAny>> {
    let out = p::list()?;
    for (index, row) in list.rows().iter().enumerate() {
        let id = unityrs_core::typetree::NodeId(u32::try_from(index)?);
        traversal.expand(
            0,
            2048_u64
                .saturating_add((row.ty.len() as u64 + row.name.len() as u64).saturating_mul(4)),
            1_u64
                .saturating_add(row.ty.len() as u64)
                .saturating_add(row.name.len() as u64),
        )?;
        let node = p::dict()?;
        if let Some((node_id, list_id)) = list.graph_identity(id) {
            p::field(&node, "_native_node", &p::uint(node_id as u64)?)?;
            p::field(&node, "_native_list", &p::uint(list_id as u64)?)?;
        }
        let children = p::list()?;
        for child in list.children(id) {
            traversal.expand(0, 64, 1)?;
            p::push(&children, &p::uint(child.0.into())?)?;
        }
        p::field(&node, "_native_children", &children)?;
        for (key, value) in [
            ("m_Level", row.level),
            ("m_ByteSize", row.byte_size),
            ("m_Version", row.version),
        ] {
            p::field(&node, key, &p::int(value.into())?)?;
        }
        p::field(&node, "m_Type", &p::string(row.ty.as_bytes())?)?;
        p::field(&node, "m_Name", &p::string(row.name.as_bytes())?)?;
        for (key, value) in [
            ("m_TypeFlags", row.type_flags.map(i64::from)),
            ("m_VariableCount", row.variable_count.map(i64::from)),
            ("m_Index", row.index.map(i64::from)),
            ("m_MetaFlag", row.meta_flag),
        ] {
            if let Some(value) = value {
                p::field(&node, key, &p::int(value)?)?;
            }
        }
        if let Some(value) = row.ref_type_hash {
            p::field(&node, "m_RefTypeHash", &p::uint(value)?)?;
        }
        p::push(&out, &node)?;
    }
    Ok(out)
}

pub(crate) fn compile_rows(object: &ObjectRef, input: &Py<PyAny>) -> PyResult<Arc<Tree>> {
    compile_rows_with(
        object.file().meter(),
        &object.file().context().limits,
        input,
    )
}

pub(crate) fn compile_rows_with(
    meter: &Arc<unityrs_core::limits::Meter>,
    limits: &unityrs_core::limits::Limits,
    input: &Py<PyAny>,
) -> PyResult<Arc<Tree>> {
    let mut rows = MeteredVec::new(meter);
    let mut staging = TreeTraversal::on(meter, limits.max_tree_depth);
    let iter = crate::python::relay::attach(|py| input.bind(py).try_iter().map(Bound::unbind))?;
    loop {
        // Only Python iteration, field extraction and encoding hold the GIL.
        // The iterator owns no flattened graph or encoded string snapshot.
        let row = crate::python::relay::attach(|py| {
            iter.bind(py)
                .clone()
                .next()
                .map(|row| {
                    let row = row?;
                    Ok::<_, PyErr>((
                        NodeRow {
                            level: row.get_item(0)?.extract()?,
                            byte_size: row.get_item(3)?.extract()?,
                            version: row.get_item(4)?.extract()?,
                            type_flags: row.get_item(5)?.extract()?,
                            variable_count: row.get_item(6)?.extract()?,
                            index: row.get_item(7)?.extract()?,
                            meta_flag: row.get_item(8)?.extract()?,
                            ref_type_hash: row.get_item(9)?.extract()?,
                            ..NodeRow::default()
                        },
                        RowText::extract(&row.get_item(1)?, &mut staging)?,
                        RowText::extract(&row.get_item(2)?, &mut staging)?,
                    ))
                })
                .transpose()
        })?;
        let Some((mut row, ty, name)) = row else {
            break;
        };
        let depth = u64::try_from(row.level)?.saturating_add(1);
        let bytes = ty.bound().saturating_add(name.bound());
        // Retain a conservative allowance for the Python iterator's path,
        // active identities and current tuple before asking for another row.
        staging.expand(depth, 512, 1_u64.saturating_add(bytes))?;
        let _encoding = meter
            .charge(256_u64.saturating_add(bytes))
            .map_err(errors::charge)?;
        let (ty, name) =
            crate::python::relay::attach(|py| Ok::<_, PyErr>((ty.bytes(py)?, name.bytes(py)?)))?;
        crate::python::relay::attach(|py| {
            py.detach(|| {
                #[cfg(test)]
                crate::row_copy_test::before_copy()?;
                rows.reserve(1).map_err(errors::charge)?;
                row.ty = RawStr::charged(&ty, meter).map_err(errors::charge)?;
                row.name = RawStr::charged(&name, meter).map_err(errors::charge)?;
                rows.push(row).map_err(errors::charge)
            })
        })?;
    }
    crate::python::relay::attach(|py| {
        py.detach(move || {
            let list = NodeList::from_rows(rows).map_err(tree_error)?;
            Tree::compile(list.into_shared().map_err(tree_error)?, limits).map_err(tree_error)
        })
    })
}

enum RowText {
    Bytes(PyBackedBytes),
    Unicode(Py<PyString>, usize),
}

impl RowText {
    fn extract(value: &Bound<'_, PyAny>, staging: &mut TreeTraversal) -> PyResult<Self> {
        if let Ok(buffer) = value.cast::<PyByteArray>() {
            // PyBackedBytes' mutable conversion allocates a Rust Arc
            // infallibly. CPython's buffer copy propagates allocation failure.
            staging.expand(0, 256_u64.saturating_add(buffer.len() as u64), 0)?;
            let bytes = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    value.py(),
                    ffi::PyBytes_FromObject(buffer.as_ptr()),
                )?
            };
            return Ok(Self::Bytes(bytes.cast_into::<PyBytes>()?.into()));
        }
        if let Ok(bytes) = value.cast::<PyBytes>() {
            return Ok(Self::Bytes(bytes.clone().into()));
        }
        let text = value.cast::<PyString>()?;
        // Read the actual Unicode size, bypassing a subclass's __len__.
        let len = unsafe { ffi::PyUnicode_GetLength(text.as_ptr()) };
        Ok(Self::Unicode(text.clone().unbind(), usize::try_from(len)?))
    }

    fn bound(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Unicode(_, len) => (*len as u64).saturating_mul(4),
        }
    }

    fn bytes(self, py: Python<'_>) -> PyResult<PyBackedBytes> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            // Use CPython's bounded encoder directly: a str subclass cannot
            // replace encode with an arbitrary allocation or callback.
            Self::Unicode(text, _) => unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyUnicode_AsEncodedString(
                        text.as_ptr(),
                        c"utf-8".as_ptr(),
                        c"surrogateescape".as_ptr(),
                    ),
                )?
                .extract()
                .map_err(Into::into)
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod allocation_tests {
    use super::*;
    pub(crate) fn bytearray_copy(py: Python<'_>) -> PyResult<()> {
        let input = py.eval(c"bytearray(8 << 20)", None, None)?;
        let meter = unityrs_core::limits::Meter::unlimited();
        let mut staging = TreeTraversal::on(&meter, 100);
        // PyBackedBytes' bytearray conversion uses an infallible Rust Arc.
        // A snapshot must instead copy with a fallible Python allocation.
        let result = crate::allocation_tests::refusing_large_input(|| {
            RowText::extract(&input, &mut staging)
        });
        assert!(result.is_ok());
        Ok(())
    }
}
