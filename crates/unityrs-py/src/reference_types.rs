// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
//! Meter reference metadata and embedded rows before handing snapshots to Python.
use crate::{python as p, traversal::TreeTraversal};
use pyo3::prelude::*;
use unityrs_core::File;

pub(crate) fn snapshot(file: &crate::files::NativeFile) -> PyResult<(Py<PyAny>, TreeTraversal)> {
    let File::Serialized(file) = &file.inner else {
        return Err(pyo3::exceptions::PyAttributeError::new_err(
            "not a serialized file",
        ));
    };
    let refs = file.ref_types().ok_or_else(|| {
        pyo3::exceptions::PyAttributeError::new_err(
            "'SerializedFile' object has no attribute 'ref_types'",
        )
    })?;
    let mut guard = TreeTraversal::on(file.meter(), file.context().limits.max_tree_depth);
    guard.expand(0, 64, 1)?;
    let result = p::list()?;
    for reference in refs {
        guard.expand(0, 1024, 1)?;
        let row = p::dict()?;
        for (key, text) in [
            ("m_ClassName", &reference.class_name),
            ("m_NameSpace", &reference.namespace),
            ("m_AssemblyName", &reference.assembly),
        ] {
            let value = match text {
                Some(text) => {
                    let len = text.as_bytes().len() as u64;
                    guard.expand(0, 256_u64.saturating_add(len.saturating_mul(4)), len)?;
                    p::string(text.as_bytes())?
                }
                None => p::none(),
            };
            p::field(&row, key, &value)?;
        }
        let rows = match &reference.nodes {
            Some(nodes) => crate::tree::rows_to_python(nodes, &mut guard)?,
            None => p::none(),
        };
        p::field(&row, "node", &rows)?;
        p::push(&result, &row)?;
    }
    Ok((result, guard))
}
