//! Synchronous warning callbacks run on the original Python caller thread.
//! The relay is stack-owned by a scoped worker; it allocates no channel,
//! closure or request buffer. No relay lock is held across Python code.
use pyo3::{prelude::*, pybacked::PyBackedBytes};
use std::{
    cell::Cell,
    sync::{Condvar, Mutex},
};
use unityrs_core::RawStr;

#[derive(Default)]
struct State {
    request: Option<(RawStr, bool)>,
    response: Option<PyResult<(RawStr, bool)>>,
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
            let Some((value, bundle)) = state.request.take() else {
                return;
            };
            drop(state);
            let answer = emit(&value, bundle);
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .response = Some(answer);
            self.changed.notify_all();
        }
    }

    fn request(&self, value: &RawStr, bundle: bool) -> PyResult<(RawStr, bool)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.request = Some((value.clone(), bundle));
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
        let (bytes, parse_typetree): (PyBackedBytes, bool) = py
            .import("unityrs.config")?
            .getattr("_warn_fallback")?
            .call1((super::string(value.as_bytes())?, bundle))?
            .extract()?;
        Ok((
            RawStr::from(unityrs_core::Bytes::from_owner(bytes)),
            parse_typetree,
        ))
    })
}

pub(crate) fn warning(value: &RawStr, bundle: bool) -> PyResult<(RawStr, bool)> {
    CURRENT.with(|slot| {
        let pointer = slot.get();
        if pointer.is_null() {
            return emit(value, bundle);
        }
        // SAFETY: enter installs only its borrowed, stack-owned Relay. Its
        // guard restores the prior pointer before that borrow ends, also on
        // unwinding. The scoped worker cannot outlive the caller's Relay.
        unsafe { &*pointer }.request(value, bundle)
    })
}
