//! Charge Unicode's UTF-8 cache before PyO3 asks `CPython` to populate it.
use crate::errors;
use pyo3::{prelude::*, pybacked::PyBackedStr, types::PyString};
use std::sync::Arc;
use unityrs_core::limits::{Charge, Meter};

pub(crate) fn snapshot(
    input: &Bound<'_, PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<(PyBackedStr, Charge)> {
    let text = input.cast::<PyString>()?;
    let len = unsafe { pyo3::ffi::PyUnicode_GetLength(text.as_ptr()) };
    let charge = meter
        .charge(256 + u64::try_from(len)?.saturating_mul(4))
        .map_err(errors::charge)?;
    Ok((input.extract()?, charge))
}
