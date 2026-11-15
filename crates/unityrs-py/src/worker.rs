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

pub(crate) fn run<T: Send>(stack: usize, f: impl FnOnce() -> PyResult<T> + Send) -> PyResult<T> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(scope, f)
            .map_err(|_| PyOSError::new_err("could not create core worker"))?;
        thread
            .join()
            .map_err(|_| PyRuntimeError::new_err("core worker failed"))?
    })
}
