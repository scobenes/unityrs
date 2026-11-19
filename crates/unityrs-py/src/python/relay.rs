//! Synchronous configuration callbacks run on the original Python caller thread.
//! The relay is stack-owned by the call running the work, on a worker or
//! inline on the caller; it allocates no channel, closure or request buffer.
//! No relay lock is held across Python code.
use pyo3::{prelude::*, pybacked::PyBackedBytes, types::PyBytes};
use std::{
    cell::Cell,
    sync::{Arc, Condvar, Mutex},
};
use unityrs_core::RawStr;
use unityrs_core::limits::Meter;

pub(crate) enum Request {
    Warning(RawStr, bool),
    DecryptKey(Arc<Meter>),
    Codec(crate::codec_bridge::Call),
    Reference(crate::reference_input::Call),
    Release(Py<PyAny>),
    Python(PythonRequest),
}

pub(crate) enum Response {
    Warning(RawStr, bool),
    DecryptKey(Option<[u8; 16]>),
    Codec(Option<crate::buffer_snapshot::BufferSnapshot>),
    Reference(Option<std::sync::Arc<unityrs_core::typetree::Tree>>),
    ReferenceMissing,
    Encoder(Py<PyAny>, usize, bool),
    Released,
    Python,
}

/// A stack-owned operation whose worker waits for dispatch before continuing.
pub(crate) struct PythonRequest {
    data: *mut (),
    invoke: unsafe fn(*mut ()),
}

// SAFETY: attach accepts only Send operations/results. The pointed-to task
// stays alive and exclusively owned by dispatch until request returns.
unsafe impl Send for PythonRequest {}

/// Attach on the original caller, including result allocation and error paths.
/// Besides explicit protocols, `CPython` allocations can run GC finalizers.
pub(crate) fn attach<T: Send>(operation: impl for<'py> FnOnce(Python<'py>) -> T + Send) -> T {
    struct Task<F, T> {
        operation: Option<F>,
        result: std::mem::MaybeUninit<std::thread::Result<T>>,
    }
    unsafe fn invoke<F, T>(data: *mut ())
    where
        F: for<'py> FnOnce(Python<'py>) -> T + Send,
        T: Send,
    {
        // SAFETY: attach supplies this exact Task type and waits synchronously.
        let task = unsafe { &mut *data.cast::<Task<F, T>>() };
        if let Some(operation) = task.operation.take() {
            task.result
                .write(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || Python::attach(operation),
                )));
        }
    }
    fn run<F, T>(operation: F) -> T
    where
        F: for<'py> FnOnce(Python<'py>) -> T + Send,
        T: Send,
    {
        let mut task = Task {
            operation: Some(operation),
            result: std::mem::MaybeUninit::uninit(),
        };
        let request = PythonRequest {
            data: std::ptr::from_mut(&mut task).cast(),
            invoke: invoke::<F, T>,
        };
        // The relay response is infallible for Python requests. A callback
        // panic is returned to the worker, so dispatch cannot strand it.
        let _ = self::request(Request::Python(request));
        // SAFETY: request returns only after invoke has run exactly once,
        // writing the result even when the operation unwinds.
        match unsafe { task.result.assume_init() } {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    if relayed() {
        run(operation)
    } else {
        Python::attach(operation)
    }
}

/// Whether this thread is a worker whose caller answers its requests. Off a
/// worker, and on a caller running its own work inline, requests are emitted
/// on this thread.
pub(crate) fn relayed() -> bool {
    let pointer = CURRENT.get();
    // SAFETY: the same scoped lifetime as request() below.
    !pointer.is_null() && !unsafe { &*pointer }.inline
}

#[derive(Default)]
struct State {
    request: Option<Request>,
    response: Option<PyResult<Response>>,
    error: Option<PyErr>,
    /// A callback panicked; it was answered with an error instead.
    panicked: bool,
    done: bool,
}

#[derive(Default)]
pub(crate) struct Relay {
    state: Mutex<State>,
    changed: Condvar,
    /// Work runs on the caller itself: requests are emitted in place, and
    /// only a callback's failure is kept for the caller.
    inline: bool,
}

thread_local! {
    static CURRENT: Cell<*const Relay> = const { Cell::new(std::ptr::null()) };
}

impl Relay {
    pub(crate) fn inline() -> Self {
        Self {
            inline: true,
            ..Self::default()
        }
    }

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
            // A panicking callback still answers, or the worker would wait
            // for its response while this caller waits for the worker.
            let answer = self.answer(request);
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .response = Some(answer);
            self.changed.notify_all();
        }
    }

    /// Emit `request`; a panic becomes an error answer, and the call's
    /// failure once its work ends (see [`Self::panicked`]).
    fn answer(&self, request: Request) -> PyResult<Response> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| emit(request))).unwrap_or_else(
            |_| {
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .panicked = true;
                Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "core worker failed",
                ))
            },
        )
    }

    /// Whether a callback panicked during the work, inline or relayed.
    pub(crate) fn panicked(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .panicked
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

fn emit(request: Request) -> PyResult<Response> {
    let request = match request {
        Request::Python(request) => {
            // SAFETY: attach keeps the task live while its worker waits;
            // dispatch holds no locks and invokes it exactly once.
            unsafe { (request.invoke)(request.data) };
            return Ok(Response::Python);
        }
        Request::Codec(call) => return crate::codec_bridge::invoke(&call),
        Request::Reference(call) => return crate::reference_input::invoke(&call),
        Request::Release(value) => {
            return Python::attach(|_| {
                drop(value);
                Ok(Response::Released)
            });
        }
        request => request,
    };
    Python::attach(|py| {
        let Request::Warning(value, bundle) = request else {
            let Request::DecryptKey(meter) = request else {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "invalid relay request",
                ));
            };
            let key = py
                .import("unityrs.helpers.ArchiveStorageManager")?
                .getattr("DECRYPT_KEY")?;
            if key.is_none() {
                return Ok(Response::DecryptKey(None));
            }
            let snapshot = crate::buffer_snapshot::BufferSnapshot::new(&key, &meter)?;
            let key = <[u8; 16]>::try_from(snapshot.as_ref()).map_err(|_| {
                pyo3::exceptions::PyValueError::new_err("AssetBundle Key length is wrong")
            })?;
            return Ok(Response::DecryptKey(Some(key)));
        };
        // This private protocol returns immutable encoded bytes. Extract the
        // exact owner, never PyBackedBytes' infallible mutable-buffer copy.
        let (bytes, parse_typetree): (Py<PyBytes>, bool) = py
            .import("unityrs.config")?
            .getattr("_warn_fallback")?
            .call1((super::string(value.as_bytes())?, bundle))?
            .extract()?;
        let bytes = PyBackedBytes::from(bytes.into_bound(py));
        Ok(Response::Warning(
            RawStr::from(unityrs_core::Bytes::from_owner(bytes)),
            parse_typetree,
        ))
    })
}

pub(crate) fn request(request: Request) -> PyResult<Response> {
    CURRENT.with(|slot| {
        let pointer = slot.get();
        if pointer.is_null() {
            return emit(request);
        }
        if !relayed() {
            // SAFETY: as below; the caller's own work, run inline.
            return unsafe { &*pointer }.answer(request);
        }
        // SAFETY: enter installs only its borrowed, stack-owned Relay. Its
        // guard restores the prior pointer before that borrow ends, also on
        // unwinding. The scoped worker cannot outlive the caller's Relay.
        unsafe { &*pointer }.request(request)
    })
}

pub(crate) fn warning(value: &RawStr, bundle: bool) -> PyResult<(RawStr, bool)> {
    match request(Request::Warning(value.clone(), bundle))? {
        Response::Warning(value, bundle) => Ok((value, bundle)),
        _ => Err(pyo3::exceptions::PyRuntimeError::new_err(
            "invalid relay response",
        )),
    }
}

pub(crate) fn decrypt_key(meter: &Arc<Meter>) -> PyResult<Option<[u8; 16]>> {
    match request(Request::DecryptKey(meter.clone()))? {
        Response::DecryptKey(key) => Ok(key),
        _ => Err(pyo3::exceptions::PyRuntimeError::new_err(
            "invalid relay response",
        )),
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;

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
