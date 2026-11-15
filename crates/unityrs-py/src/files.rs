// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/SerializedFile.py, UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! File metadata and children; no Python attachment surrounds a core lookup.
use crate::{errors, object::NativeObject, python as p};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::File;

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeFile {
    pub(crate) inner: File,
}
impl NativeFile {
    pub(crate) fn new(inner: File) -> Self {
        Self { inner }
    }
}
#[pymethods]
impl NativeFile {
    #[getter]
    fn identity(&self, py: Python<'_>) -> usize {
        py.detach(|| match &self.inner {
            File::Serialized(f) => Arc::as_ptr(f) as usize,
            File::Bundle(f) => Arc::as_ptr(f) as usize,
            File::Web(f) => Arc::as_ptr(f) as usize,
            File::Raw(f) => Arc::as_ptr(f) as usize,
        })
    }
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut staging = unityrs_core::limits::Charge::zero(self.inner.meter());
            staging.grow(1024).map_err(errors::charge)?;
            let d = p::dict()?;
            let (kind, name) = match &self.inner {
                File::Serialized(f) => ("SerializedFile", f.name()),
                File::Bundle(f) => ("BundleFile", f.name()),
                File::Web(f) => ("WebFile", f.name()),
                File::Raw(f) => (
                    if f.reader_kind() == unityrs_core::stream::ReaderKind::Stream {
                        "EndianBinaryReader_Stream"
                    } else {
                        "EndianBinaryReader"
                    },
                    &f.name,
                ),
            };
            staging
                .grow((name.as_bytes().len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::field(&d, "kind", &p::string(kind.as_bytes())?)?;
            p::field(&d, "name", &p::string(name.as_bytes())?)?;
            if let File::Raw(f) = &self.inner {
                p::field(&d, "length", &p::uint(f.data.len() as u64)?)?;
            }
            if let File::Serialized(f) = &self.inner {
                p::field(
                    &d,
                    "resolved_version",
                    &match f.version() {
                        Some(v) => p::string(v.version.raw().as_bytes())?,
                        None => p::none(),
                    },
                )?;
                p::field(
                    &d,
                    "unity_version",
                    &match f.unity_version() {
                        Some(s) => p::string(s.as_bytes())?,
                        None => p::none(),
                    },
                )?;
                p::field(
                    &d,
                    "target_platform",
                    &match f.target_platform() {
                        Some(i) => p::int(i.into())?,
                        None => p::none(),
                    },
                )?;
                p::field(&d, "enable_type_tree", &p::boolean(f.enable_type_tree())?)?;
                p::field(&d, "big_id_enabled", &p::int(f.big_id_enabled().into())?)?;
                let h = p::dict()?;
                let header = f.header();
                for (k, v) in [
                    ("metadata_size", i64::from(header.metadata_size)),
                    ("file_size", header.file_size),
                    ("version", header.version.into()),
                    ("data_offset", header.data_offset),
                ] {
                    p::field(&h, k, &p::int(v)?)?;
                }
                p::field(&d, "header", &h)?;
            }
            if let File::Bundle(f) = &self.inner {
                let h = f.header();
                p::field(&d, "signature", &p::string(h.signature.as_bytes())?)?;
                p::field(&d, "version", &p::uint(h.format.into())?)?;
            }
            Ok(d)
        })
    }
    fn entries(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut staging = unityrs_core::limits::Charge::zero(self.inner.meter());
            staging.grow(64).map_err(errors::charge)?;
            let result = p::list()?;
            for slot in self.inner.container_files() {
                staging
                    .grow(256 + (slot.name.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                let file = self
                    .inner
                    .container_entry(slot.dir_index)
                    .map_err(errors::core)?;
                let row = p::list()?;
                p::push(&row, &p::string(slot.name.as_bytes())?)?;
                let f = Python::attach(|py| Py::new(py, NativeFile::new(file)).map(Py::into_any))?;
                p::push(&row, &f)?;
                p::push(&result, &row)?;
            }
            Ok(result)
        })
    }
    fn objects(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut staging = unityrs_core::limits::Charge::zero(self.inner.meter());
            staging.grow(64).map_err(errors::charge)?;
            let result = p::list()?;
            if let File::Serialized(f) = &self.inner {
                for inner in f.objects() {
                    staging.grow(128).map_err(errors::charge)?;
                    let obj = Python::attach(|py| {
                        Py::new(py, NativeObject::new(inner)).map(Py::into_any)
                    })?;
                    p::push(&result, &obj)?;
                }
            }
            Ok(result)
        })
    }
    fn raw_read(&self, py: Python<'_>, position: i128, size: i64) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let File::Raw(f) = &self.inner else {
                return Err(pyo3::exceptions::PyAttributeError::new_err(
                    "not a raw reader",
                ));
            };
            let kind = f.reader_kind();
            let start = kind
                .seek(position)
                .map_err(|s| errors::stream(s.exception()))?;
            let (range, next) = kind
                .read(f.data.len(), start, size)
                .map_err(|s| errors::stream(s.exception()))?;
            let bytes = f
                .data
                .get(range)
                .ok_or_else(|| pyo3::exceptions::PyEOFError::new_err("raw span"))?;
            let _copy = f
                .meter()
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            let result = p::dict()?;
            p::field(&result, "data", &p::bytes(bytes)?)?;
            p::field(&result, "position", &p::wide_int(next)?)?;
            Ok(result)
        })
    }
    fn raw_data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| match &self.inner {
            File::Raw(f) => {
                let _copy = f
                    .meter()
                    .charge(unityrs_core::limits::heap_cost(f.data.len() as u64))
                    .map_err(errors::charge)?;
                p::bytes(&f.data)
            }
            _ => Err(pyo3::exceptions::PyAttributeError::new_err(
                "file is not a raw reader",
            )),
        })
    }
}
