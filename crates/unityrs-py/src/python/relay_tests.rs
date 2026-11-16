//! Reentrant finalization and unwind handling at the worker relay boundary.
use pyo3::{prelude::*, types::PyDict};

#[test]
fn worker_decrefs_finalize_on_caller_thread() -> PyResult<()> {
    let _serial = crate::tests::serial();
    unityrs_testkit::guard::finishes("worker decref reentry under DEADLOCK_GUARD", || {
        Python::attach(|py| {
            let scope = PyDict::new(py);
            py.run(
                c"
import threading
lock = threading.RLock()
caller = threading.get_ident()
seen = []
class Owner(bytes):
    def __del__(self):
        same = threading.get_ident() == caller
        seen.append(same)
        if same:
            with lock:
                seen.append(True)
lock.acquire()
",
                Some(&scope),
                None,
            )?;
            let owner = py.eval(c"Owner(b'input')", Some(&scope), None)?.unbind();
            py.detach(move || {
                crate::worker::run(2 << 20, move || {
                    // PyO3 queues decrefs without attaching. The next output
                    // attachment must drain that queue on the original caller.
                    drop(owner);
                    crate::python::list()
                })
            })?;
            py.run(
                c"lock.release(); assert seen == [True, True], seen",
                Some(&scope),
                None,
            )
        })
    })
}

#[test]
fn python_callback_unwind_returns_worker_error_without_deadlock() -> PyResult<()> {
    let _serial = crate::tests::serial();
    unityrs_testkit::guard::finishes("relay unwind under DEADLOCK_GUARD", || {
        Python::attach(|py| {
            let error: PyResult<()> = py.detach(|| {
                crate::worker::run(2 << 20, || {
                    super::attach(|py| {
                        // Unwind from a Rust callback, after a Python reentry.
                        py.eval(c"__import__('threading').get_ident()", None, None)?;
                        std::panic::resume_unwind(Box::new("synthetic callback unwind"))
                    })
                })
            });
            assert!(
                matches!(error, Err(error) if error.is_instance_of::<pyo3::exceptions::PyRuntimeError>(py))
            );
            let caller = std::thread::current().id();
            let thread = py.detach(|| {
                crate::worker::run(2 << 20, || {
                    Ok(super::attach(|_| std::thread::current().id()))
                })
            })?;
            assert_eq!(thread, caller);
            Ok(())
        })
    })
}
