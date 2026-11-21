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
    fn monobehaviour_head_nodes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let file = self.inner.file();
        let version = file.version().ok_or_else(|| {
            crate::errors::new::<pyo3::exceptions::PyNotImplementedError>("Unsupported comparison")
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
            .ok_or_else(|| {
                crate::errors::new_with::<pyo3::exceptions::PyKeyError>(|py| {
                    crate::python::held::int(py, 114)
                })
            })?;
        let mut guard =
            crate::traversal::TreeTraversal::on(file.meter(), file.context().limits.max_tree_depth);
        guard.expand(u64::from(tree.tree.depth()), 0, 0)?;
        let rows = crate::tree::rows_to_python(tree.tree.list(), &mut guard)?;
        p::held::with_guard(py, (rows, guard))
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
                return Err(crate::errors::new::<pyo3::exceptions::PyValueError>(
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
                &errors::message(&error),
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
    fn nodes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let nodes = py.detach(|| crate::tree::object_nodes(&self.inner))?;
        p::held::with_guard(py, nodes)
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
    /// `reader.read_typetree()`'s dict, or with `wrap` its object, read
    /// directly; `None` where the caller takes the full path (`crate::direct`).
    fn parse_direct(
        &self,
        py: Python<'_>,
        reader: &Bound<'_, PyAny>,
        check_read: bool,
        wrap: bool,
    ) -> PyResult<Option<Py<PyAny>>> {
        crate::direct::read(py, &self.inner, reader, check_read, wrap)
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
            crate::worker::run_read(&self.inner, tree.is_some(), || {
                // Within the decode, as it reads: a read that reacquires
                // the GIL fails the handshake.
                #[cfg(test)]
                if let Err(error) = crate::row_copy_test::before_decode() {
                    return Ok(Err(error));
                }
                let parsed = self
                    .inner
                    .read_typetree(tree.as_ref(), check_read)
                    .map_err(|error| Failure::parse(error, self.inner.info().byte_size > 0))?;
                // The parsed tree is dropped here, on the worker when read there.
                Ok::<_, Failure>((|| {
                    let _staging = meter.charge(256).map_err(errors::charge)?;
                    let mut charge = unityrs_core::limits::Charge::zero(meter);
                    let value = crate::value_render::convert(&parsed, meter, &mut charge, false)?;
                    Ok::<_, PyErr>((value, parsed.bytes_read))
                })())
            })
        })?;
        let (value, count) = crate::worker_error::finish(native)??;
        p::held::tuple_of(py, &[value.into_bound(py), p::held::int(py, count)?]).map(Bound::unbind)
    }
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        use p::held as h;
        static KEYS: pyo3::sync::PyOnceLock<Vec<Py<PyAny>>> = pyo3::sync::PyOnceLock::new();
        let (embedded, i) = py.detach(|| {
            (
                self.inner
                    .serialized_type()
                    .is_some_and(|ty| ty.nodes.is_some()),
                self.inner.info().clone(),
            )
        });
        // Built attached, with keys made once: callers read it on every parse.
        let keys = KEYS.get_or_try_init(py, || {
            [
                "_has_embedded_nodes",
                "path_id",
                "class_id",
                "type_id",
                "byte_size",
                "byte_start",
                "is_destroyed",
                "is_stripped",
            ]
            .into_iter()
            .map(|key| h::string(py, key.as_bytes()).map(Bound::unbind))
            .collect::<PyResult<Vec<_>>>()
        })?;
        let optional =
            |value: Option<i64>| value.map_or_else(|| Ok(h::none(py)), |n| h::int(py, n));
        let values = [
            h::boolean(py, embedded)?,
            h::int(py, i.path_id)?,
            h::int(py, i.class_id.into())?,
            h::int(py, i.type_id.into())?,
            h::int(py, i.byte_size.into())?,
            // byte_start can exceed i64: add the signed header offset in Python.
            p::wide_int(i.byte_start)?.into_bound(py),
            optional(i.is_destroyed.map(i64::from))?,
            optional(i.is_stripped.map(i64::from))?,
        ];
        let d = h::dict(py)?;
        for (key, value) in keys.iter().zip(&values) {
            h::set(&d, key.bind(py), value)?;
        }
        Ok(d.unbind())
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
            crate::worker::run_read(&self.inner, tree.is_some(), || {
                let (parsed, progress) = self
                    .inner
                    .read_object_fields_with_nodes(tree.as_ref())
                    .map_err(|error| Failure::parse(error, self.inner.info().byte_size > 0))?;
                Ok::<_, Failure>(object_result(&self.inner, parsed, progress))
            })
        })?;
        crate::worker_error::finish(native)?
    }
    fn parse(&self, py: Python<'_>, check_read: bool) -> PyResult<Py<PyAny>> {
        self.parse_with_position(py, check_read, None)
            .and_then(|result| result.bind(py).get_item(0).map(Bound::unbind))
    }
}

/// `object_data`'s result, on the worker. The value is rendered, then the
/// parsed tree dropped (on the worker's stack) before the node metadata
/// adds its staging; only the node lists stay.
fn object_result(
    inner: &ObjectRef,
    parsed: unityrs_core::Parsed,
    progress: unityrs_core::walk::FieldProgress,
) -> PyResult<Py<PyAny>> {
    use p::held as h;
    let meter = inner.file().meter();
    let mut charge = unityrs_core::limits::Charge::zero(meter);
    charge.grow(128).map_err(errors::charge)?;
    let value = crate::value_render::convert(&parsed, meter, &mut charge, true)?;
    let mut lists = unityrs_core::limits::MeteredVec::with_capacity(meter, parsed.lists().len())
        .map_err(errors::charge)?;
    for list in parsed.lists() {
        lists.push(list.clone()).map_err(errors::charge)?;
    }
    let bytes_read = parsed.bytes_read;
    drop(parsed);
    let nodes = crate::object_nodes::convert(lists.as_slice(), &mut charge)?;
    let references =
        crate::object_nodes::references(lists.as_slice(), inner.file().ref_types(), &mut charge)?;
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
            crate::python::relay::attach(|py| -> PyResult<PyErr> {
                let mut args = Vec::new();
                args.try_reserve_exact(names.len()).map_err(|_| {
                    crate::errors::new::<pyo3::exceptions::PyMemoryError>("diagnostic")
                })?;
                for name in &names {
                    args.push(h::string(py, name.as_bytes())?);
                }
                let message = h::string(py, b"Referenced type not found: {} {} {}")?
                    .getattr(h::string(py, b"format")?.cast_into::<pyo3::types::PyString>()?)?
                    .call1(h::tuple_of(py, &args)?.cast_into::<pyo3::types::PyTuple>()?)?;
                Ok(pyo3::exceptions::PyValueError::new_err(message.unbind()))
            })?
        } else {
            errors::parse_reader(
                &unityrs_core::walk::ParseError::Read(error),
                inner.info().byte_size > 0,
            )
        };
        Some(error)
    } else {
        None
    };
    crate::python::relay::attach(move |py| {
        let error = error.map_or_else(|| h::none(py), |error| error.value(py).clone().into_any());
        let construction = Bound::new(py, crate::construction::Construction { charge })?;
        let result = h::list(py)?;
        for item in [
            value.into_bound(py),
            nodes.into_bound(py),
            references.into_bound(py),
            h::int(py, bytes_read)?,
            h::uint(py, progress.completed as u64)?,
            error,
            construction.into_any(),
        ] {
            h::push(&result, &item)?;
        }
        Ok(result.unbind())
    })
}
