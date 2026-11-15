// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Dict mode and metadata (files/ObjectReader.py:44-130, files/ObjectReader.py:244-262,
//! files/ObjectReader.py:278-283, files/ObjectReader.py:313-314).
use crate::{errors, python as p, value};
use pyo3::prelude::*;
use unityrs_core::ObjectRef;

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeObject {
    pub(crate) inner: ObjectRef,
}
impl NativeObject {
    pub(crate) fn new(inner: ObjectRef) -> Self {
        Self { inner }
    }
}
#[pymethods]
impl NativeObject {
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let d = p::dict()?;
            let i = self.inner.info();
            for (key, val) in [
                ("path_id", i.path_id),
                ("class_id", i.class_id.into()),
                ("type_id", i.type_id.into()),
                ("byte_size", i.byte_size.into()),
            ] {
                p::field(&d, key, &p::int(val)?)?;
            }
            // byte_start can exceed i64: add the signed header offset in Python.
            let start = p::wide_int(i.byte_start)?;
            p::field(&d, "byte_start", &start)?;
            for (key, val) in [
                ("is_destroyed", i.is_destroyed.map(i64::from)),
                ("is_stripped", i.is_stripped.map(i64::from)),
            ] {
                p::field(
                    &d,
                    key,
                    &match val {
                        Some(n) => p::int(n)?,
                        None => p::none(),
                    },
                )?;
            }
            Ok(d)
        })
    }
    fn raw_data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let bytes = self.inner.raw_data().map_err(errors::core)?;
            let _copy = self
                .inner
                .file()
                .meter()
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    }
    fn parse(&self, py: Python<'_>, check_read: bool) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                let parsed = self
                    .inner
                    .parse_as_dict(check_read)
                    .map_err(|e| errors::parse_reader(&e, self.inner.info().byte_size > 0))?;
                value::convert(&parsed, self.inner.file().meter())
            })
        })
    }
}
