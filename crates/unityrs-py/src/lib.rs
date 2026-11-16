//! Python abi3 boundary. Core work always runs detached from Python.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use pyo3::prelude::*;
#[cfg(test)]
mod allocation_tests;
mod buffer_snapshot;
mod class_tpk;
mod construction;
mod environment;
mod errors;
mod exports;
mod files;
mod images;
mod object;
mod object_nodes;
mod path_snapshot;
mod python;
#[cfg(test)]
mod row_copy_test;
mod save;
mod save_snapshot;
mod settings;
mod text_snapshot;
mod tpk;
mod traversal;
mod tree;
mod value;
mod value_input;
#[cfg(test)]
mod value_input_tests;
mod worker;

#[pymodule(name = "_native")]
fn native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    exports::register(m)?;
    images::register(m)?;
    m.add_class::<save::NativeCab>()?;
    m.add_function(wrap_pyfunction!(class_tpk::class_tpk_lz4, m)?)?;
    m.add_class::<environment::NativeEnvironment>()?;
    m.add_class::<files::NativeFile>()?;
    m.add_class::<object::NativeObject>()?;
    m.add_class::<settings::Budget>()?;
    m.add_class::<settings::Limits>()?;
    m.add_class::<tpk::NativeTpk>()?;
    m.add_class::<traversal::TreeTraversal>()?;
    #[cfg(test)]
    m.add_class::<row_copy_test::RowCopyBarrier>()?;
    m.add_function(wrap_pyfunction!(tpk::mit_common_strings, m)?)?;
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
