//! Synchronous callbacks run on the original Python caller thread.
//! The relay is stack-owned by a scoped worker; it allocates no channel,
//! closure or request buffer. No relay lock is held across Python code.
use pyo3::{prelude::*, pybacked::PyBackedBytes, types::PyBytes};
use std::{
    cell::Cell,
    sync::{Condvar, Mutex},
};
use unityrs_core::RawStr;

pub(crate) enum Request {
    Warning(RawStr, bool),
    Codec(crate::codec_bridge::Call),
    Reference(crate::reference_input::Call),
    Release(Py<PyAny>),
}

pub(crate) enum Response {
    Warning(RawStr, bool),
    Codec(Option<crate::buffer_snapshot::BufferSnapshot>),
    Reference(Option<std::sync::Arc<unityrs_core::typetree::Tree>>),
    ReferenceMissing,
    Encoder(Py<PyAny>, usize, bool),
    Released,
}

#[derive(Default)]
struct State {
    request: Option<Request>,
    response: Option<PyResult<Response>>,
    error: Option<PyErr>,
    done: bool,
}

#[derive(Default)]
pub(crate) struct Relay {
    state: Mutex<State>,
    changed: Condvar,
}

thread_local! {
    static CURRENT: Cell<*const Relay> = const { Cell::new(std::ptr::null()) };
}

impl Relay {
    pub(crate) fn enter<T>(&self, operation: impl FnOnce() -> T) -> T {
        struct Guard<'a> {
            relay: &'a Relay,
            previous: *const Relay,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                CURRENT.set(self.previous);
                self.relay
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .done = true;
                self.relay.changed.notify_all();
            }
        }
        let _guard = Guard {
            relay: self,
            previous: CURRENT.replace(std::ptr::from_ref(self)),
        };
        operation()
    }

    pub(crate) fn dispatch(&self) {
        loop {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while state.request.is_none() && !state.done {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            let Some(request) = state.request.take() else {
                return;
            };
            drop(state);
            let answer = match request {
                Request::Warning(value, bundle) => {
                    emit(&value, bundle).map(|(v, b)| Response::Warning(v, b))
                }
                Request::Codec(call) => crate::codec_bridge::invoke(&call),
                Request::Reference(call) => crate::reference_input::invoke(&call),
                Request::Release(value) => Python::attach(|_| {
                    drop(value);
                    Ok(Response::Released)
                }),
            };
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .response = Some(answer);
            self.changed.notify_all();
        }
    }

    pub(crate) fn take_error(&self) -> Option<PyErr> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error
            .take()
    }

    fn request(&self, request: Request) -> PyResult<Response> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.request = Some(request);
        self.changed.notify_all();
        loop {
            if let Some(answer) = state.response.take() {
                return answer;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

fn emit(value: &RawStr, bundle: bool) -> PyResult<(RawStr, bool)> {
    Python::attach(|py| {
        // This private protocol returns immutable encoded bytes. Extract the
        // exact owner, never PyBackedBytes' infallible mutable-buffer copy.
        let (bytes, parse_typetree): (Py<PyBytes>, bool) = py
            .import("unityrs.config")?
            .getattr("_warn_fallback")?
            .call1((super::string(value.as_bytes())?, bundle))?
            .extract()?;
        let bytes = PyBackedBytes::from(bytes.into_bound(py));
        Ok((
            RawStr::from(unityrs_core::Bytes::from_owner(bytes)),
            parse_typetree,
        ))
    })
}

pub(crate) fn warning(value: &RawStr, bundle: bool) -> PyResult<(RawStr, bool)> {
    match request(Request::Warning(value.clone(), bundle))? {
        Response::Warning(value, bundle) => Ok((value, bundle)),
        _ => Err(pyo3::exceptions::PyRuntimeError::new_err(
            "invalid warning relay response",
        )),
    }
}

pub(crate) fn request(request: Request) -> PyResult<Response> {
    CURRENT.with(|slot| {
        let pointer = slot.get();
        if pointer.is_null() {
            return match request {
                Request::Warning(value, bundle) => {
                    emit(&value, bundle).map(|(v, b)| Response::Warning(v, b))
                }
                Request::Codec(call) => crate::codec_bridge::invoke(&call),
                Request::Reference(call) => crate::reference_input::invoke(&call),
                Request::Release(value) => Python::attach(|_| {
                    drop(value);
                    Ok(Response::Released)
                }),
            };
        }
        // SAFETY: enter installs only its borrowed, stack-owned Relay. Its
        // guard restores the prior pointer before that borrow ends, also on
        // unwinding. The scoped worker cannot outlive the caller's Relay.
        unsafe { &*pointer }.request(request)
    })
}

pub(crate) fn fail(error: PyErr) {
    CURRENT.with(|slot| {
        let pointer = slot.get();
        if !pointer.is_null() {
            // SAFETY: the same scoped lifetime as request() above.
            let relay = unsafe { &*pointer };
            let mut state = relay
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.error.is_none() {
                state.error = Some(error);
            }
        }
    });
}
