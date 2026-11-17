//! Python threads may have tiny stacks. Parser work uses the core's advertised
//! stack allowance; the caller is detached while this scoped worker runs.
use pyo3::{
    PyResult,
    exceptions::{PyOSError, PyRuntimeError},
};

/// Loading also recurses through eager containers and legacy tree nodes.
/// Add their frames: a container may parse a serialized tree at its leaf.
pub(crate) fn load_stack(limits: &unityrs_core::limits::Limits) -> usize {
    let per_level = if cfg!(debug_assertions) {
        64 << 10
    } else {
        16 << 10
    };
    limits.stack_needed().saturating_add(
        (limits.max_container_depth as usize)
            .saturating_add(limits.max_tree_depth as usize)
            .saturating_add(1)
            .saturating_mul(per_level),
    )
}

/// Return native data (including unconverted errors). Callers translate and
/// build Python objects after this joins; Python operations needed during work
/// use the caller relay, including warnings, allocations and finalizers.
pub(crate) fn run<T: Send>(stack: usize, f: impl FnOnce() -> T + Send) -> PyResult<T> {
    let relay = crate::python::relay::Relay::default();
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(scope, || relay.enter(f))
            .map_err(|_| PyOSError::new_err("could not create core worker"))?;
        relay.dispatch();
        let result = thread
            .join()
            .map_err(|_| PyRuntimeError::new_err("core worker failed"))?;
        match relay.take_error() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    })
}

pub(crate) fn python_warning(
    value: &unityrs_core::RawStr,
    bundle: bool,
) -> PyResult<(unityrs_core::RawStr, bool)> {
    crate::python::relay::warning(value, bundle)
}
