//! Charge Python path encoding and reserve native storage before copying.
use crate::errors;
use pyo3::{
    prelude::*,
    types::{PyBytes, PyString},
};
use std::{path::PathBuf, sync::Arc};
use unityrs_core::limits::{Charge, Meter};

pub(crate) fn snapshot(
    input: &Bound<'_, PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<(PathBuf, Charge)> {
    let value = input.py().import("os")?.call_method1("fspath", (input,))?;
    let len = if let Ok(text) = value.cast::<PyString>() {
        usize::try_from(unsafe { pyo3::ffi::PyUnicode_GetLength(text.as_ptr()) })?
    } else {
        value.cast::<PyBytes>()?.as_bytes().len()
    };
    // Both the filesystem encoding and the native path can be live at once.
    let allowance = 256 + (len as u64).saturating_mul(8);
    let mut charge = meter.charge(allowance).map_err(errors::charge)?;
    let encoded = value
        .py()
        .import("os")?
        .call_method1("fsencode", (&value,))?;
    let bytes = encoded.cast::<PyBytes>()?.as_bytes();
    // A user-defined encoder can return more than the ordinary UTF-8 bound.
    charge
        .grow((256 + (bytes.len() as u64).saturating_mul(2)).saturating_sub(allowance))
        .map_err(errors::charge)?;
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes)
    };
    #[cfg(not(unix))]
    let path = std::ffi::OsStr::new(
        std::str::from_utf8(bytes)
            .map_err(|_| pyo3::exceptions::PyValueError::new_err("path must be UTF-8"))?,
    );
    let mut out = PathBuf::new();
    out.try_reserve_exact(path.len())
        .map_err(|_| pyo3::exceptions::PyMemoryError::new_err("path allocation refused"))?;
    out.push(path);
    Ok((out, charge))
}
