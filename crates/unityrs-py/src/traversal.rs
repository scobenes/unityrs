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
    value_limit: u64,
    zero_limit: u64,
    depth_limit: u32,
}

impl TreeTraversal {
    pub(crate) fn provided(py: Python<'_>, guard: Option<Py<Self>>) -> PyResult<Py<Self>> {
        guard.map_or_else(|| Py::new(py, Self::new(None, None)), Ok)
    }

    pub(crate) fn shared_step(guard: &Py<Self>, memory: u64, work: u64) -> PyResult<()> {
        Python::attach(|py| guard.try_borrow_mut(py)?.expand(0, memory, work))
    }

    pub(crate) fn on(meter: &Arc<Meter>, depth_limit: u32) -> Self {
        Self {
            inner: CoreTraversal::new(meter, depth_limit),
            value_limit: unityrs_core::limits::Limits::default().max_values_per_object,
            zero_limit: unityrs_core::limits::Limits::default().max_zero_size_elements,
            depth_limit: unityrs_core::limits::Limits::default().max_value_depth,
        }
    }

    pub(crate) fn with_limits(meter: &Arc<Meter>, limits: &unityrs_core::limits::Limits) -> Self {
        Self {
            inner: CoreTraversal::new(meter, limits.max_tree_depth),
            value_limit: limits.max_values_per_object,
            zero_limit: limits.max_zero_size_elements,
            depth_limit: limits.max_value_depth,
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
        Self::with_limits(
            &Meter::for_input(&limits, budget, Reservation::Input),
            &limits,
        )
    }

    fn step(&mut self, depth: u64, memory: u64, work: u64) -> PyResult<()> {
        self.expand(depth, memory, work)
    }

    fn value_depth(&self, depth: u64) -> PyResult<()> {
        if depth > u64::from(self.depth_limit) {
            return Err(errors::core(
                unityrs_core::LimitError::refused(
                    unityrs_core::LimitKind::ValueDepth,
                    depth,
                    u64::from(self.depth_limit),
                )
                .into(),
            ));
        }
        Ok(())
    }

    fn read_values(&mut self, py: Python<'_>, total: u64, added: u64) -> PyResult<()> {
        py.detach(|| {
            if total > self.value_limit {
                return Err(errors::core(
                    unityrs_core::LimitError::refused(
                        unityrs_core::LimitKind::ValuesPerObject,
                        total,
                        self.value_limit,
                    )
                    .into(),
                ));
            }
            self.expand(0, 0, added.saturating_mul(unityrs_core::walk::VALUE_WORK))
        })
    }

    fn read_zero_size(&mut self, py: Python<'_>, total: u64) -> PyResult<()> {
        py.detach(|| {
            if total > self.zero_limit {
                return Err(errors::core(
                    unityrs_core::LimitError::refused(
                        unityrs_core::LimitKind::ZeroSizeElements,
                        total,
                        self.zero_limit,
                    )
                    .into(),
                ));
            }
            Ok(())
        })
    }
}
