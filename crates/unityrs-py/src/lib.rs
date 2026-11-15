//! Python abi3 boundary. Core work always runs detached from Python.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use pyo3::prelude::*;
mod environment;
mod errors;
mod files;
mod object;
mod python;
mod settings;
mod value;
mod worker;

#[pymodule(name = "_native")]
fn native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<environment::NativeEnvironment>()?;
    m.add_class::<files::NativeFile>()?;
    m.add_class::<object::NativeObject>()?;
    m.add_class::<settings::Budget>()?;
    m.add_class::<settings::Limits>()?;
    m.add(
        "_class_ids",
        m.py().detach(|| {
            let result = python::dict()?;
            for &(id, name) in unityrs_core::classid::CLASS_NAMES {
                python::set(
                    &result,
                    &python::string(name.as_bytes())?,
                    &python::int(id.into())?,
                )?;
            }
            Ok::<_, PyErr>(result)
        })?,
    )?;
    m.add("LimitError", m.py().get_type::<errors::LimitError>())?;
    m.add("TypeTreeError", m.py().get_type::<errors::TypeTreeError>())?;
    Ok(())
}

#[cfg(test)]
mod tests;
