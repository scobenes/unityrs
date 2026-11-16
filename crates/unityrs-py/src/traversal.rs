//! One charging policy for Python helper expansion and native tree staging.
use crate::{
    errors,
    settings::{Budget, Limits},
};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Meter, TreeTraversal as CoreTraversal},
};

#[pyclass(module = "unityrs._native")]
pub(crate) struct TreeTraversal {
    inner: CoreTraversal,
}

impl TreeTraversal {
    pub(crate) fn provided(py: Python<'_>, guard: Option<Py<Self>>) -> PyResult<Py<Self>> {
        guard.map_or_else(|| Py::new(py, Self::new(None, None)), Ok)
    }

    pub(crate) fn shared_step(guard: &Py<Self>, memory: u64, work: u64) -> PyResult<()> {
        crate::python::relay::attach(|py| guard.try_borrow_mut(py)?.expand(0, memory, work))
    }

    pub(crate) fn on(meter: &Arc<Meter>, depth_limit: u32) -> Self {
        Self {
            inner: CoreTraversal::new(meter, depth_limit),
        }
    }

    pub(crate) fn expand(&mut self, depth: u64, memory: u64, work: u64) -> PyResult<()> {
        self.inner.step(depth, memory, work).map_err(errors::charge)
    }
}

#[pymethods]
impl TreeTraversal {
    #[new]
    #[pyo3(signature = (limits=None, budget=None))]
    fn new(limits: Option<&Limits>, budget: Option<&Budget>) -> Self {
        let limits = limits.map_or_else(unityrs_core::limits::Limits::default, |v| v.inner);
        let budget: Arc<dyn unityrs_core::budget::MemoryBudget> = budget.map_or_else(
            || Arc::new(Unbounded) as Arc<dyn unityrs_core::budget::MemoryBudget>,
            |v| v.inner.clone(),
        );
        Self::on(
            &Meter::for_input(&limits, budget, Reservation::Input),
            limits.max_tree_depth,
        )
    }

    fn step(&mut self, depth: u64, memory: u64, work: u64) -> PyResult<()> {
        self.expand(depth, memory, work)
    }
}
