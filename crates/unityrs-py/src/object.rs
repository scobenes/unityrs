// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Dict mode and metadata (files/ObjectReader.py:44-130, files/ObjectReader.py:244-262,
//! files/ObjectReader.py:278-283, files/ObjectReader.py:313-314).
use crate::worker_error::Failure;
use crate::{errors, python as p};
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
    fn monobehaviour_head_nodes(
        &self,
        py: Python<'_>,
    ) -> PyResult<(Py<PyAny>, crate::traversal::TreeTraversal)> {
        let file = self.inner.file();
        let version = file.version().ok_or_else(|| {
            pyo3::exceptions::PyNotImplementedError::new_err("Unsupported comparison")
        })?;
        let request = unityrs_core::provider::TreeRequest {
            class_id: 114,
            version: &version.version,
            script: None,
        };
        let tree = py
            .detach(|| {
                crate::worker::run(file.context().limits.stack_needed(), || {
                    file.context().providers.lookup(&request)
                })
            })?
            .map_err(|error| errors::parse(unityrs_core::walk::ParseError::Provider(error)))?
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(114))?;
        let mut guard =
            crate::traversal::TreeTraversal::on(file.meter(), file.context().limits.max_tree_depth);
        guard.expand(u64::from(tree.tree.depth()), 0, 0)?;
        Ok((
            crate::tree::rows_to_python(tree.tree.list(), &mut guard)?,
            guard,
        ))
    }

    fn set_raw_data(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        crate::save::object_set_raw_data(py, self, data)
    }
    fn save_metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        crate::save::object_metadata(py, self)
    }
    #[pyo3(signature = (value, rows, offset=0, endian=None))]
    fn save_typetree(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        rows: &Bound<'_, PyAny>,
        offset: usize,
        endian: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        // The Python caller owns the environment's RLock. Getters, numeric
        // protocols, and row iteration must run on that thread so they can
        // reenter the owning reader. Row copying and compilation detach on
        // this thread, attaching only for Python extraction; final native building
        // and writing use a worker.
        let tree = crate::tree::compile_rows(&self.inner, rows.as_unbound())?;
        let file = self.inner.file();
        let meter = file.meter();
        let limits = &file.context().limits;
        let value = value.as_unbound();
        let snapshot = py.detach(|| {
            crate::value_input::for_tree_snapshot(
                value,
                meter,
                limits,
                &tree,
                file.ref_types().unwrap_or_default(),
            )
        })?;
        let endian = endian
            .map(|v| crate::text_snapshot::snapshot(v, meter))
            .transpose()?;
        let endian = match endian.as_ref().map(|(text, _)| text.as_str()) {
            Some(">") => unityrs_core::stream::Endian::Big,
            Some("<") => unityrs_core::stream::Endian::Little,
            Some(_) => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "unsupported writer endian",
                ));
            }
            None => file.header().endian,
        };
        let (out, result) = py.detach(move || {
            crate::worker::run(limits.stack_needed(), move || {
                let mut out = unityrs_core::budget::DecodeBuf::new(
                    meter,
                    0,
                    limits.max_input_memory,
                    unityrs_core::LimitKind::InputMemory,
                )
                .map_err(Failure::charge)?;
                meter.count_decode(offset as u64).map_err(Failure::charge)?;
                let mut left = offset;
                while left > 0 {
                    let n = left.min(4096);
                    out.extend_from_slice([0_u8; 4096].get(..n).unwrap_or_default())
                        .map_err(Failure::charge)?;
                    left -= n;
                }
                let (parsed, lengths) = snapshot.build_writer()?;
                let ctx = unityrs_core::walk::WriteCtx {
                    endian,
                    ref_types: file.ref_types_view(),
                    rules: unityrs_core::walk::WriteRules::UnityPy,
                    limits,
                };
                let result = unityrs_core::walk::write::write_at_with_lengths(
                    &parsed,
                    &tree,
                    &ctx,
                    &mut out,
                    0,
                    &|value| lengths.get(value, parsed.value()),
                );
                Ok::<_, Failure>((out, result))
            })
        })??;
        result.map_err(|error| match error {
            unityrs_core::walk::WriteError::Charge(e) => errors::charge(e),
            unityrs_core::walk::WriteError::Limit(e) => errors::core(e.into()),
            _ => crate::exports::named_error(
                error.exceptions().first().copied().unwrap_or("ValueError"),
                errors::message(&error),
            ),
        })?;
        let data = out.as_slice().get(offset..).unwrap_or_default();
        let _copy = meter.charge(data.len() as u64).map_err(errors::charge)?;
        p::bytes(data)
    }
    fn tree_traversal(&self) -> crate::traversal::TreeTraversal {
        crate::traversal::TreeTraversal::with_limits(
            self.inner.file().meter(),
            &self.inner.file().context().limits,
        )
    }
    fn nodes(&self, py: Python<'_>) -> PyResult<(Py<PyAny>, crate::traversal::TreeTraversal)> {
        py.detach(|| crate::tree::object_nodes(&self.inner))
    }
    fn parse_nodes(
        &self,
        py: Python<'_>,
        check_read: bool,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.parse_with_position(py, check_read, Some(rows))
            .and_then(|result| result.bind(py).get_item(0).map(Bound::unbind))
    }
    #[pyo3(signature = (check_read, rows=None))]
    fn parse_with_position(
        &self,
        py: Python<'_>,
        check_read: bool,
        rows: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        // Row iteration can invoke caller code while its environment lock is held.
        let tree = rows
            .map(|rows| crate::tree::compile_rows(&self.inner, rows.as_unbound()))
            .transpose()?;
        let meter = self.inner.file().meter();
        let native = py.detach(|| {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                let parsed = self
                    .inner
                    .read_typetree(tree.as_ref(), check_read)
                    .map_err(|error| Failure::parse(error, self.inner.info().byte_size > 0))?;
                // One attachment builds the whole value; the parsed tree is
                // still dropped here, on the advertised stack.
                Ok::<_, Failure>(crate::python::relay::attach(|py| {
                    let _staging = meter.charge(256).map_err(errors::charge)?;
                    let mut charge = unityrs_core::limits::Charge::zero(meter);
                    let value =
                        crate::value_render::render(py, &parsed, meter, &mut charge, false)?;
                    let count = p::held::int(py, parsed.bytes_read)?;
                    Ok::<_, PyErr>(
                        pyo3::types::PyTuple::new(py, [value, count])?
                            .into_any()
                            .unbind(),
                    )
                }))
            })
        })?;
        crate::worker_error::finish(native)?
    }
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (embedded, i) = py.detach(|| {
            (
                self.inner
                    .serialized_type()
                    .is_some_and(|ty| ty.nodes.is_some()),
                self.inner.info().clone(),
            )
        });
        {
            let d = p::dict()?;
            p::field(&d, "_has_embedded_nodes", &p::boolean(embedded)?)?;
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
        }
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
    fn object_data(&self, py: Python<'_>, rows: Option<&Bound<'_, PyAny>>) -> PyResult<Py<PyAny>> {
        let tree = rows
            .map(|rows| crate::tree::compile_rows(&self.inner, rows.as_unbound()))
            .transpose()?;
        let native = py.detach(|| {
            crate::worker::run(self.inner.file().context().limits.stack_needed(), || {
                let (parsed, progress) = self
                    .inner
                    .read_object_fields_with_nodes(tree.as_ref())
                    .map_err(|error| Failure::parse(error, self.inner.info().byte_size > 0))?;
                // One attachment builds the whole result; the parsed tree is
                // still dropped here, on the advertised stack.
                Ok::<_, Failure>(crate::python::relay::attach(|py| {
                    object_output(py, &self.inner, &parsed, progress)
                }))
            })
        })?;
        crate::worker_error::finish(native)?
    }
    fn parse(&self, py: Python<'_>, check_read: bool) -> PyResult<Py<PyAny>> {
        self.parse_with_position(py, check_read, None)
            .and_then(|result| result.bind(py).get_item(0).map(Bound::unbind))
    }
}

/// `object_data`'s result, built on an attached caller.
fn object_output(
    py: Python<'_>,
    inner: &ObjectRef,
    parsed: &unityrs_core::Parsed,
    progress: unityrs_core::walk::FieldProgress,
) -> PyResult<Py<PyAny>> {
    let meter = inner.file().meter();
    let mut charge = unityrs_core::limits::Charge::zero(meter);
    charge.grow(128).map_err(errors::charge)?;
    let fields = crate::value_render::render(py, parsed, meter, &mut charge, true)?.unbind();
    let bytes_read = parsed.bytes_read;
    let result = p::list()?;
    p::push(&result, &fields)?;
    p::push(
        &result,
        &crate::object_nodes::convert(parsed.lists(), &mut charge)?,
    )?;
    p::push(
        &result,
        &crate::object_nodes::references(parsed.lists(), inner.file().ref_types(), &mut charge)?,
    )?;
    p::push(&result, &p::int(bytes_read)?)?;
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
            crate::python::relay::attach(|py| -> PyResult<PyErr> {
                let message = pyo3::types::PyString::new(py, "Referenced type not found: {} {} {}")
                    .call_method1("format", args)?;
                Ok(pyo3::exceptions::PyValueError::new_err(message.unbind()))
            })?
        } else {
            errors::parse_reader(
                &unityrs_core::walk::ParseError::Read(error),
                inner.info().byte_size > 0,
            )
        };
        crate::python::relay::attach(|py| error.value(py).clone().into_any().unbind())
    } else {
        p::none()
    };
    p::push(&result, &error)?;
    let construction = crate::python::relay::attach(|py| {
        Py::new(py, crate::construction::Construction { charge }).map(Py::into_any)
    })?;
    p::push(&result, &construction)?;
    Ok(result)
}
