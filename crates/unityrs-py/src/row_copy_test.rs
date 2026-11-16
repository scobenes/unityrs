//! Test-only handshake at the detached row-copy boundary; absent from wheels.
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::{
    cell::RefCell,
    sync::{Arc, Condvar, Mutex},
};

thread_local! {
    static VALUE_ARMED: RefCell<Option<Arc<State>>> = const { RefCell::new(None) };
    static ARMED: RefCell<Option<Arc<State>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct State {
    // (copy started, Python acknowledged)
    phase: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl State {
    fn wait(&self, acknowledged: bool) -> PyResult<()> {
        let phase = self.phase.lock().map_err(lock_error)?;
        let (_phase, timeout) = self
            .changed
            .wait_timeout_while(
                phase,
                unityrs_testkit::guard::DEADLOCK_GUARD,
                |&mut (started, ack)| {
                    if acknowledged { !ack } else { !started }
                },
            )
            .map_err(lock_error)?;
        if timeout.timed_out() {
            return Err(PyRuntimeError::new_err(if acknowledged {
                "row copy timed out waiting for Python acknowledgement (GIL held)"
            } else {
                "Python worker timed out waiting for native row copy"
            }));
        }
        Ok(())
    }
}

fn lock_error<T>(_: T) -> PyErr {
    PyRuntimeError::new_err("row-copy barrier lock poisoned")
}

#[pyclass(frozen, name = "_RowCopyBarrier")]
#[derive(Default)]
pub(crate) struct RowCopyBarrier(Arc<State>);

#[pymethods]
impl RowCopyBarrier {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    fn arm(&self) {
        ARMED.with(|armed| *armed.borrow_mut() = Some(self.0.clone()));
    }

    fn arm_value(&self) {
        VALUE_ARMED.with(|armed| *armed.borrow_mut() = Some(self.0.clone()));
    }

    fn wait_for_copy(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.0.wait(false))
    }

    fn acknowledge(&self) -> PyResult<()> {
        self.0.phase.lock().map_err(lock_error)?.1 = true;
        self.0.changed.notify_all();
        Ok(())
    }
}

pub(crate) fn before_copy() -> PyResult<()> {
    let Some(state) = ARMED.with(|armed| armed.borrow_mut().take()) else {
        return Ok(());
    };
    state.phase.lock().map_err(lock_error)?.0 = true;
    state.changed.notify_all();
    // Deliberately do not detach here: this must fail if the caller holds the GIL.
    state.wait(true)
}

pub(crate) fn before_value() -> PyResult<()> {
    let Some(state) = VALUE_ARMED.with(|armed| armed.borrow_mut().take()) else {
        return Ok(());
    };
    state.phase.lock().map_err(lock_error)?.0 = true;
    state.changed.notify_all();
    state.wait(true)
}

#[test]
fn value_snapshot_releases_gil_on_calling_thread() -> PyResult<()> {
    let _serial = crate::tests::serial();
    unityrs_testkit::guard::finishes("native value staging and competing Python thread", || {
        Python::attach(|py| {
            let module = PyModule::new(py, "unityrs._native")?;
            super::native(&module)?;
            py.import("sys")?
                .getattr("modules")?
                .set_item("unityrs._native", module)?;
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            for directory in [root.join("python"), root.join("../../tools/parity")] {
                py.import("sys")?
                    .getattr("path")?
                    .call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
            }
            py.import("binding_y4_round5")?
                .getattr("observe_snapshot_threads")?
                .call1((py.import("unityrs")?,))?;
            Ok(())
        })
    })
}

#[test]
fn row_copy_releases_gil_between_python_extractions() -> PyResult<()> {
    check_row_copy(false)
}

#[test]
fn writer_row_copy_releases_gil_on_calling_thread() -> PyResult<()> {
    check_row_copy(true)
}

fn check_row_copy(save: bool) -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let module = PyModule::new(py, "unityrs._native")?;
        super::native(&module)?;
        py.import("sys")?
            .getattr("modules")?
            .set_item("unityrs._native", module)?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = py.import("sys")?.getattr("path")?;
        for directory in [root.join("python"), root.join("python/tests")] {
            paths.call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
        }
        py.import("test_helpers")?
            .getattr("test_row_copy_releases_gil_between_python_extractions")?
            .call1((save,))?;
        Ok(())
    })
}
