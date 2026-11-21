//! Budgets are Rust atomic counters; no Python callbacks run under core locks.
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{budget::Counting, limits::Limits as CoreLimits};

#[pyclass(frozen, skip_from_py_object, module = "unityrs")]
#[derive(Clone)]
pub(crate) struct Budget {
    pub(crate) inner: Arc<Counting>,
    limit: u64,
}
#[pymethods]
impl Budget {
    #[new]
    fn new(py: Python<'_>, limit: u64) -> Self {
        py.detach(|| Self {
            inner: Arc::new(Counting::new(limit)),
            limit,
        })
    }
    #[getter]
    fn limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        crate::python::held::uint(py, self.limit).map(Bound::unbind)
    }
    #[getter]
    fn used(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let used = py.detach(|| self.inner.used());
        crate::python::held::uint(py, used).map(Bound::unbind)
    }
}

#[pyclass(frozen, skip_from_py_object, module = "unityrs")]
#[derive(Clone)]
pub(crate) struct Limits {
    pub(crate) inner: CoreLimits,
}
#[pymethods]
impl Limits {
    #[new]
    #[pyo3(signature = (*, max_input_memory=None, max_input_bytes=None, max_block_output=None, max_decode_work=None, max_inflate=None, max_container_depth=None, max_tree_depth=None, max_value_depth=None, max_values_per_object=None, max_zero_size_elements=None, max_prealloc=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        max_input_memory: Option<u64>,
        max_input_bytes: Option<u64>,
        max_block_output: Option<u64>,
        max_decode_work: Option<u64>,
        max_inflate: Option<u64>,
        max_container_depth: Option<u32>,
        max_tree_depth: Option<u32>,
        max_value_depth: Option<u32>,
        max_values_per_object: Option<u64>,
        max_zero_size_elements: Option<u64>,
        max_prealloc: Option<usize>,
    ) -> Self {
        py.detach(|| {
            let mut inner = CoreLimits::default();
            macro_rules! apply { ($($field:ident),*) => { $(if let Some(v) = $field { inner.$field = v; })* }; }
            apply!(max_input_memory, max_input_bytes, max_block_output, max_decode_work, max_inflate, max_container_depth, max_tree_depth, max_value_depth, max_values_per_object, max_zero_size_elements, max_prealloc);
            Self { inner }
        })
    }
    #[allow(
        clippy::cast_lossless,
        reason = "one macro handles usize and u32 counters"
    )]
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let value = py.detach(|| {
            macro_rules! fields { ($($field:ident),*) => { match name { $(stringify!($field) => Ok(self.inner.$field as u64),)* _ => Err(crate::errors::new::<pyo3::exceptions::PyAttributeError>("unknown limit")) } }; }
            fields!(max_input_memory, max_input_bytes, max_block_output, max_decode_work, max_inflate, max_container_depth, max_tree_depth, max_value_depth, max_values_per_object, max_zero_size_elements, max_prealloc)
        })?;
        crate::python::held::uint(py, value).map(Bound::unbind)
    }
}
