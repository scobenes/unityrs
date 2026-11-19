//! Test adapter over the value rendering used by object parsing.
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{Parsed, limits::Meter};

pub(crate) fn convert(parsed: &Parsed, meter: &Arc<Meter>) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| {
        let mut charge = unityrs_core::limits::Charge::zero(meter);
        crate::value_render::render(py, parsed, meter, &mut charge, false).map(Bound::unbind)
    })
}
