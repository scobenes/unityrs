//! Test adapter over the native handoff used by object parsing.
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{Parsed, limits::Meter};

pub(crate) fn convert(parsed: &Parsed, meter: &Arc<Meter>) -> PyResult<Py<PyAny>> {
    let mut plan = crate::python::plan::Plan::new(meter);
    plan.root = crate::value_plan::convert(parsed, meter, &mut plan, false)?;
    plan.render()
}
