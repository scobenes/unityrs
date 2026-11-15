//! Keep conversion and reconstruction charged together until Python handoff.
use crate::errors;
use pyo3::prelude::*;
use unityrs_core::limits::Charge;

#[pyclass(module = "unityrs._native")]
pub(crate) struct Construction {
    pub(crate) charge: Charge,
}

#[pymethods]
impl Construction {
    fn reserve(&mut self, py: Python<'_>, memory: u64, work: u64) -> PyResult<()> {
        py.detach(|| {
            self.charge
                .meter()
                .count_decode(work)
                .map_err(errors::charge)?;
            self.charge.grow(memory).map_err(errors::charge)
        })
    }
}
