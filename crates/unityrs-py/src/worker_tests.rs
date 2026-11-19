//! Where work runs: inline on a caller whose stack covers it, else on a
//! pooled worker, with the same errors, relay and panic answers either way.
#![allow(clippy::panic, reason = "a failed test thread fails the test")]
use pyo3::{exceptions::PyRuntimeError, exceptions::PyValueError, prelude::*};
use std::thread::ThreadId;

/// Run `body` attached on a new thread with `stack` bytes of stack.
pub(crate) fn on_thread<T: Send + 'static>(
    stack: usize,
    body: impl FnOnce(Python<'_>) -> T + Send + 'static,
) -> T {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || Python::attach(body))
        .and_then(|thread| thread.join().map_err(|_| std::io::Error::other("panicked")))
        .unwrap_or_else(|error| std::panic::panic_any(error.to_string()))
}

fn ids(py: Python<'_>) -> PyResult<(ThreadId, ThreadId, ThreadId)> {
    let caller = std::thread::current().id();
    let work = py.detach(|| super::run(1 << 20, || std::thread::current().id()))?;
    let callback = py.detach(|| {
        super::run(1 << 20, || {
            crate::python::relay::attach(|_| std::thread::current().id())
        })
    })?;
    Ok((caller, work, callback))
}

#[test]
fn a_caller_with_room_runs_its_work_inline() -> PyResult<()> {
    let _serial = crate::tests::serial();
    let (caller, work, callback) = on_thread(256 << 20, ids)?;
    // Only Linux and macOS can tell a thread's stack; elsewhere work pools.
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert_eq!(work, caller);
    } else {
        assert_ne!(work, caller);
    }
    assert_eq!(callback, caller);
    Ok(())
}

#[test]
fn a_small_stack_runs_on_a_pooled_worker_and_relays_callbacks() -> PyResult<()> {
    let _serial = crate::tests::serial();
    let (caller, work, callback) = on_thread(256 << 10, ids)?;
    assert_ne!(work, caller);
    assert_eq!(callback, caller);
    assert!(!super::idle().is_empty(), "the worker parks for reuse");
    Python::attach(super::_release_workers);
    assert!(super::idle().is_empty(), "the fork hook joins idle workers");
    Ok(())
}

#[test]
fn errors_and_panics_answer_alike_inline_and_pooled() {
    let _serial = crate::tests::serial();
    for stack in [256 << 20, 256 << 10] {
        on_thread(stack, |py| {
            let failed = py.detach(|| {
                super::run(1 << 20, || {
                    crate::python::relay::fail(PyValueError::new_err("callback failed"));
                    7
                })
            });
            assert!(matches!(failed, Err(ref e) if e.is_instance_of::<PyValueError>(py)));
            let panicked = py.detach(|| {
                super::run(1 << 20, || -> u8 {
                    std::panic::resume_unwind(Box::new("synthetic worker panic"))
                })
            });
            assert!(matches!(panicked, Err(ref e) if e.is_instance_of::<PyRuntimeError>(py)));
            let again = py.detach(|| super::run(1 << 20, || 3));
            assert!(matches!(again, Ok(3)));
        });
    }
}
