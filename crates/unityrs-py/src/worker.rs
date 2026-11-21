//! Python threads may have tiny stacks. Parser work gets the core's advertised
//! stack allowance: inline on a caller (never the main thread) whose fixed
//! stack is known to cover the allowance, else on a pooled worker that parks
//! between calls (a thread per call mapped and tore down its stack every
//! time). Either way the caller is detached while the work runs, and a panic
//! is the call's error, also a callback's (which is answered with an error,
//! so neither side waits on).
use pyo3::{
    PyResult,
    exceptions::{PyOSError, PyRuntimeError},
    prelude::*,
};
use std::sync::{Mutex, PoisonError, mpsc};

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

/// A call's work with its borrows erased. `run` waits for its acknowledgement
/// before the borrowed frame ends, as a scoped thread's join does.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// A parked worker thread: one job at a time, each acknowledged on `done`.
struct Worker {
    stack: usize,
    process: u32,
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

/// Idle workers kept for reuse. A call takes one whose stack is at least its
/// allowance, or creates one; past this many idle, a returned worker exits.
const MAX_IDLE: usize = 8;
/// A new worker's least stack: the default limits' parse and load allowances
/// round up to it, so they share workers. Untouched pages stay uncommitted.
const MIN_STACK: usize = 8 << 20;

static IDLE: Mutex<Vec<Worker>> = Mutex::new(Vec::new());

fn idle() -> std::sync::MutexGuard<'static, Vec<Worker>> {
    let mut idle = IDLE.lock().unwrap_or_else(PoisonError::into_inner);
    let process = std::process::id();
    if idle.iter().any(|worker| worker.process != process) {
        // A child forked from C inherits the handles but not their threads.
        // Leak them untouched: their channels' state is the parent's.
        let (own, stale) = std::mem::take(&mut *idle)
            .into_iter()
            .partition(|worker| worker.process == process);
        std::mem::forget::<Vec<Worker>>(stale);
        *idle = own;
    }
    idle
}

fn spawn(stack: usize) -> std::io::Result<Worker> {
    let size = stack
        .checked_next_power_of_two()
        .unwrap_or(stack)
        .max(MIN_STACK);
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (ack, done) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .stack_size(size)
        .spawn(move || {
            for job in inbox {
                job();
                if ack.send(()).is_err() {
                    break;
                }
            }
        })?;
    Ok(Worker {
        stack: size,
        process: std::process::id(),
        jobs,
        done,
        thread,
    })
}

/// Hand `job` to an idle worker with enough stack, else to a new one.
fn start(stack: usize, mut job: Job) -> PyResult<Worker> {
    loop {
        let found = {
            let mut idle = idle();
            idle.iter()
                .position(|worker| worker.stack >= stack)
                .map(|at| idle.swap_remove(at))
        };
        let Some(worker) = found else { break };
        match worker.jobs.send(job) {
            Ok(()) => return Ok(worker),
            Err(mpsc::SendError(back)) => job = back,
        }
    }
    let worker = spawn(stack)
        .map_err(|_| crate::errors::new::<PyOSError>("could not create core worker"))?;
    worker
        .jobs
        .send(job)
        .map_err(|_| crate::errors::new::<PyRuntimeError>("core worker failed"))?;
    Ok(worker)
}

/// Return native data (including unconverted errors). Callers translate and
/// build Python objects after this returns; Python operations needed during
/// work use the caller relay, including warnings, allocations and finalizers.
pub(crate) fn run<T: Send>(stack: usize, f: impl FnOnce() -> T + Send) -> PyResult<T> {
    /// Waits for the worker even if dispatch unwinds: the job borrows this frame.
    struct Pending(Option<Worker>);
    impl Pending {
        fn finish(&mut self) -> Option<Worker> {
            let worker = self.0.take()?;
            worker.done.recv().ok().map(|()| worker)
        }
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            self.finish();
        }
    }
    if !crate::python::relay::relayed() && crate::stack::covers(stack) {
        return inline(f);
    }
    let relay = crate::python::relay::Relay::default();
    let mut out = None;
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(|| {
        out = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || relay.enter(f),
        )));
    });
    // SAFETY: only the lifetime is erased. The job runs at most once; it is
    // either dropped unrun before `start` returns or run by the worker, which
    // drops it before acknowledging. `Pending` receives that acknowledgement
    // (or the worker's exit) before `relay` and `out` leave scope, also when
    // dispatch unwinds, so no borrow outlives this frame.
    let job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Job>(job) };
    let mut pending = Pending(Some(start(stack, job)?));
    relay.dispatch();
    let worker = pending.finish();
    let result = match out {
        Some(Ok(result)) => {
            if let Some(worker) = worker {
                let mut idle = idle();
                if idle.len() < MAX_IDLE {
                    idle.push(worker);
                }
            }
            result
        }
        // A panicked worker retires: each panic leaves a fresh thread.
        Some(Err(_)) | None => {
            return Err(crate::errors::new::<PyRuntimeError>("core worker failed"));
        }
    };
    if relay.panicked() {
        return Err(crate::errors::new::<PyRuntimeError>("core worker failed"));
    }
    match relay.take_error() {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

/// Work whose native stack does not grow with its input (no recursion:
/// the type-tree read and its rendering keep their depth on the heap) runs
/// inline on any thread, the main thread included, with no stack check. On
/// a worker serving a caller elsewhere it takes a worker as [`run`] does, so
/// its callbacks still reach that caller.
pub(crate) fn flat<T: Send>(f: impl FnOnce() -> T + Send) -> PyResult<T> {
    if crate::python::relay::relayed() {
        return run(MIN_STACK, f);
    }
    inline(f)
}

/// An object's type-tree read (or its tree's resolution), `rows` given or
/// not: [`flat`] where all of it is, else on a worker of the parse
/// allowance. Flat: UnityPyBoost's reader (`byte_size > 0`), the tree
/// compiled from the given rows or the type record's own nodes, the value's
/// pinning and rendering. Recursive, so on a worker: the pure-Python reader
/// (`byte_size == 0`), and a tree a provider makes (a class database's, a
/// generator's).
pub(crate) fn run_read<T: Send>(
    object: &unityrs_core::ObjectRef,
    rows: bool,
    f: impl FnOnce() -> T + Send,
) -> PyResult<T> {
    let embedded = rows
        || object
            .serialized_type()
            .is_some_and(|ty| ty.nodes.is_some());
    if object.info().byte_size > 0 && embedded {
        flat(f)
    } else {
        run(object.file().context().limits.stack_needed(), f)
    }
}

/// `f` on this thread, its callbacks emitted in place, with a worker's
/// answers to a panic or a callback's failure.
fn inline<T>(f: impl FnOnce() -> T) -> PyResult<T> {
    let relay = crate::python::relay::Relay::inline();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relay.enter(f)))
        .map_err(|_| crate::errors::new::<PyRuntimeError>("core worker failed"))?;
    if relay.panicked() {
        return Err(crate::errors::new::<PyRuntimeError>("core worker failed"));
    }
    match relay.take_error() {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

/// `os.register_at_fork(before=...)`: idle workers exit and are joined, so a
/// forked child starts with no thread of ours (the core holds none between
/// calls, as before workers were pooled). A call in flight stays unsupported.
#[pyfunction]
pub(crate) fn _release_workers(py: Python<'_>) {
    let workers = std::mem::take(&mut *idle());
    py.detach(|| {
        for worker in workers {
            let Worker { jobs, thread, .. } = worker;
            drop(jobs);
            let _ = thread.join();
        }
    });
}

pub(crate) fn python_warning(
    value: &unityrs_core::RawStr,
    bundle: bool,
) -> PyResult<(unityrs_core::RawStr, bool)> {
    crate::python::relay::warning(value, bundle)
}

#[cfg(test)]
#[path = "worker_tests.rs"]
pub(crate) mod tests;
