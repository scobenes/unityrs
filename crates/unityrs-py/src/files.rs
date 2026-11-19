// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/SerializedFile.py, UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! File metadata and children; no Python attachment surrounds a core lookup.
use crate::{errors, object::NativeObject, python as p};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::File;

#[pyclass(frozen, weakref, module = "unityrs._native")]
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
    fn reference_types(
        &self,
        py: Python<'_>,
    ) -> PyResult<(Py<PyAny>, crate::traversal::TreeTraversal)> {
        py.detach(|| crate::reference_types::snapshot(self))
    }
    fn constructor_reader_state(&self, py: Python<'_>) -> (i128, Option<char>) {
        py.detach(|| match &self.inner {
            File::Serialized(file) => (
                file.constructor_position(),
                Some(
                    if file.header().endian == unityrs_core::stream::Endian::Little {
                        '<'
                    } else {
                        '>'
                    },
                ),
            ),
            File::Bundle(file) => (file.constructor_position(), None),
            File::Web(file) if file.packer() == unityrs_core::files::WebPacker::None => {
                (file.constructor_position(), Some('<'))
            }
            _ => (0, None),
        })
    }
    fn serialized_reader_info(&self, py: Python<'_>) -> PyResult<(usize, char, bool, i128)> {
        py.detach(|| {
            let File::Serialized(file) = &self.inner else {
                return Err(pyo3::exceptions::PyAttributeError::new_err(
                    "not a serialized reader",
                ));
            };
            Ok((
                file.data().len(),
                if file.header().endian == unityrs_core::stream::Endian::Little {
                    '<'
                } else {
                    '>'
                },
                file.reader_kind() != unityrs_core::stream::ReaderKind::Memory,
                file.constructor_position(),
            ))
        })
    }
    fn serialized_read(&self, py: Python<'_>, position: i128, size: i64) -> PyResult<Py<PyAny>> {
        let (bytes, next, _copy) = py.detach(|| {
            let File::Serialized(file) = &self.inner else {
                return Err(pyo3::exceptions::PyAttributeError::new_err(
                    "not a serialized reader",
                ));
            };
            let kind = file.reader_kind();
            let start = kind
                .seek(position)
                .map_err(|e| errors::stream(e.exception()))?;
            let (span, next) = kind
                .read(file.data().len(), start, size)
                .map_err(|e| errors::stream(e.exception()))?;
            let bytes = file
                .data()
                .get(span)
                .ok_or_else(|| pyo3::exceptions::PyEOFError::new_err("serialized reader span"))?;
            file.meter()
                .count_decode(bytes.len() as u64)
                .map_err(errors::charge)?;
            let copy = file
                .meter()
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64) + 256)
                .map_err(errors::charge)?;
            Ok::<_, PyErr>((bytes, next, copy))
        })?;
        // Built attached, once the core's part is done.
        let items = [
            p::held::bytes(py, bytes)?,
            p::wide_int(next)?.into_bound(py),
        ];
        Ok(pyo3::types::PyTuple::new(py, items)?.into_any().unbind())
    }
    fn save_serialized_snapshot(
        &self,
        py: Python<'_>,
        metadata: &Bound<'_, PyAny>,
        rows: &Bound<'_, PyAny>,
        externals: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        crate::save_snapshot::serialized(py, self, metadata, rows, externals)
    }
    fn save_bundle_entries(
        &self,
        py: Python<'_>,
        mode: &Bound<'_, PyAny>,
        rows: &Bound<'_, PyAny>,
        value: Option<&Bound<'_, PyAny>>,
        flags: (u32, u16),
        header: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (mode, _charge) = crate::text_snapshot::snapshot(mode, self.inner.meter())?;
        crate::save::save_bundle_entries(py, self, &mode, rows, value, flags, header)
    }
    fn save_web_entries(
        &self,
        py: Python<'_>,
        rows: &Bound<'_, PyAny>,
        packer: Option<&Bound<'_, PyAny>>,
        signature: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        crate::save::save_web_entries(py, self, rows, packer, signature)
    }
    fn save(
        &self,
        py: Python<'_>,
        mode: &Bound<'_, PyAny>,
        value: Option<&Bound<'_, PyAny>>,
        data_flag: u32,
        block_info_flag: u16,
        signature: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (mode, _mode_charge) = crate::text_snapshot::snapshot(mode, self.inner.meter())?;
        let (signature, _signature_charge) =
            crate::text_snapshot::snapshot(signature, self.inner.meter())?;
        crate::save::file_save(
            py,
            self,
            &mode,
            value,
            data_flag,
            block_info_flag,
            &signature,
        )
    }
    fn save_metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        crate::save::file_metadata(py, self)
    }
    fn is_changed(&self, py: Python<'_>) -> bool {
        crate::save::file_is_changed(py, self)
    }
    fn mark_changed(&self, py: Python<'_>) {
        crate::save::file_mark_changed(py, self);
    }
    fn get_writeable_cab(
        &self,
        py: Python<'_>,
        name: &Bound<'_, PyAny>,
        guid: Option<[u8; 16]>,
    ) -> PyResult<Option<crate::save::NativeCab>> {
        crate::save::file_get_writeable_cab(py, self, name, guid)
    }
    fn create_cab(
        &self,
        py: Python<'_>,
        name: &Bound<'_, PyAny>,
        flags: u32,
        endian: char,
    ) -> PyResult<crate::save::NativeCab> {
        crate::save::file_create_cab(py, self, name, flags, endian)
    }
    fn tree_traversal(&self) -> crate::traversal::TreeTraversal {
        let limits = match &self.inner {
            File::Serialized(f) => f.context().limits,
            File::Bundle(f) => f.context().limits,
            File::Web(f) => f.context().limits,
            File::Raw(_) => unityrs_core::limits::Limits::default(),
        };
        crate::traversal::TreeTraversal::with_limits(self.inner.meter(), &limits)
    }
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
                    if f.reader_kind() == unityrs_core::stream::ReaderKind::Memory {
                        "EndianBinaryReader"
                    } else {
                        "EndianBinaryReader_Stream"
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
                p::field(
                    &h,
                    "endian",
                    &p::string(if header.endian == unityrs_core::stream::Endian::Little {
                        b"<"
                    } else {
                        b">"
                    })?,
                )?;
                p::field(
                    &h,
                    "reserved",
                    &match header.reserved {
                        Some(v) => p::bytes(&v)?,
                        None => p::none(),
                    },
                )?;
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
    fn externals(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut charge = unityrs_core::limits::Charge::zero(self.inner.meter());
            charge.grow(64).map_err(errors::charge)?;
            let out = p::list()?;
            if let File::Serialized(file) = &self.inner {
                for external in file.externals() {
                    charge
                        .grow(
                            512 + (external.path.len() as u64).saturating_mul(8)
                                + external
                                    .temp_empty
                                    .as_ref()
                                    .map_or(0, |v| (v.len() as u64).saturating_mul(4)),
                        )
                        .map_err(errors::charge)?;
                    let row = p::dict()?;
                    p::field(&row, "path", &p::string(external.path.as_bytes())?)?;
                    p::field(
                        &row,
                        "temp_empty",
                        &match &external.temp_empty {
                            Some(v) => p::string(v.as_bytes())?,
                            None => p::none(),
                        },
                    )?;
                    p::field(
                        &row,
                        "guid",
                        &match &external.guid {
                            Some(v) => p::bytes(v.as_slice())?,
                            None => p::none(),
                        },
                    )?;
                    p::field(
                        &row,
                        "type",
                        &match external.kind {
                            Some(v) => p::int(i64::from(v))?,
                            None => p::none(),
                        },
                    )?;
                    p::push(&out, &row)?;
                }
            }
            Ok(out)
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
                let f = crate::python::relay::attach(|py| {
                    Py::new(py, NativeFile::new(file)).map(Py::into_any)
                })?;
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
                    let obj = crate::python::relay::attach(|py| {
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
    fn validate_external_path(&self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<()> {
        let path = crate::buffer_snapshot::BufferSnapshot::new(path, self.inner.meter())?;
        py.detach(|| {
            self.inner
                .meter()
                .count_decode(path.len() as u64)
                .map_err(errors::charge)?;
            if path.contains(&0) || path.len() > unityrs_core::stream::STRING_TO_NULL_WINDOW {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "CAB external path out of range",
                ));
            }
            Ok(())
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
