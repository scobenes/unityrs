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
    fn tree_traversal(&self) -> crate::traversal::TreeTraversal {
        crate::traversal::TreeTraversal::on(
            self.inner.file().meter(),
            self.inner.file().context().limits.max_tree_depth,
        )
    }
    fn nodes(&self, py: Python<'_>) -> PyResult<(Py<PyAny>, crate::traversal::TreeTraversal)> {
        py.detach(|| {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                crate::tree::object_nodes(&self.inner)
            })
        })
    }
    fn parse_nodes(
        &self,
        py: Python<'_>,
        check_read: bool,
        rows: Py<PyAny>,
    ) -> PyResult<Py<PyAny>> {
        py.detach(move || {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                let tree = crate::tree::compile_rows(&self.inner, &rows)?;
                let parsed = self
                    .inner
                    .read_typetree(Some(&tree), check_read)
                    .map_err(|e| errors::parse_reader(&e, self.inner.info().byte_size > 0))?;
                value::convert(&parsed, self.inner.file().meter())
            })
        })
    }
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
    #[pyo3(signature = (rows=None))]
    fn object_data(&self, py: Python<'_>, rows: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        py.detach(move || {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                let tree = rows
                    .as_ref()
                    .map(|rows| crate::tree::compile_rows(&self.inner, rows))
                    .transpose()?;
                let (parsed, progress) = self
                    .inner
                    .read_object_fields_with_nodes(tree.as_ref())
                    .map_err(|e| errors::parse_reader(&e, self.inner.info().byte_size > 0))?;
                let meter = self.inner.file().meter();
                let mut charge = unityrs_core::limits::Charge::zero(meter);
                charge.grow(128).map_err(errors::charge)?;
                let result = p::list()?;
                p::push(
                    &result,
                    &value::convert_fields(&parsed, meter, &mut charge)?,
                )?;
                p::push(
                    &result,
                    &crate::object_nodes::convert(&parsed, &mut charge)?,
                )?;
                p::push(
                    &result,
                    &crate::object_nodes::references(
                        &parsed,
                        self.inner.file().ref_types(),
                        &mut charge,
                    )?,
                )?;
                p::push(&result, &p::int(parsed.bytes_read)?)?;
                p::push(&result, &p::uint(progress.completed as u64)?)?;
                let error = if let Some(error) = progress.error {
                    let error = if let Some(names) = progress.reference_names {
                        charge
                            .grow(
                                256 + names
                                    .iter()
                                    .map(|s| s.as_bytes().len() as u64)
                                    .sum::<u64>()
                                    .saturating_mul(8),
                            )
                            .map_err(errors::charge)?;
                        let [class, namespace, assembly] = names;
                        let args = (
                            p::string(class.as_bytes())?,
                            p::string(namespace.as_bytes())?,
                            p::string(assembly.as_bytes())?,
                        );
                        Python::attach(|py| -> PyResult<PyErr> {
                            let message = pyo3::types::PyString::new(
                                py,
                                "Referenced type not found: {} {} {}",
                            )
                            .call_method1("format", args)?;
                            Ok(pyo3::exceptions::PyValueError::new_err(message.unbind()))
                        })?
                    } else {
                        errors::parse_reader(
                            &unityrs_core::walk::ParseError::Read(error),
                            self.inner.info().byte_size > 0,
                        )
                    };
                    Python::attach(|py| error.value(py).clone().into_any().unbind())
                } else {
                    p::none()
                };
                p::push(&result, &error)?;
                let construction = Python::attach(|py| {
                    Py::new(py, crate::construction::Construction { charge }).map(Py::into_any)
                })?;
                p::push(&result, &construction)?;
                Ok(result)
            })
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
