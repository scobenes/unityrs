//! Explicit execution ownership, outside `Controller::drive_local`'s unwind
//! boundary. Drop never joins: the host parks this owner after hand-over;
//! cooperative tests release their holds and call `quiesce` explicitly.
use crate::{
    controller::{Controller, SetupError},
    job::End,
    spec::model::Spec,
    sys::Wake,
    worker::{self, Dispatch},
};
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::JoinHandle,
};
struct StartupGate(Arc<(Mutex<bool>, Condvar)>);
impl StartupGate {
    /// Some platforms (Darwin) allocate a std mutex's and condvar's OS
    /// storage lazily, on first use, by whichever thread gets there first.
    /// Lock both here, on the constructing thread before the running head,
    /// with a zero-duration wait, so no worker allocates them after the head.
    fn new() -> Self {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        {
            let (released, changed) = &*gate;
            let primed = released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = changed.wait_timeout(primed, std::time::Duration::ZERO);
        }
        Self(gate)
    }
    fn wait(gate: &(Mutex<bool>, Condvar)) {
        let (released, changed) = gate;
        let mut released = released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = changed
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}
impl Drop for StartupGate {
    fn drop(&mut self) {
        let (released, changed) = &*self.0;
        *released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }
}
/// The result itself is the readiness flag, set before the pool wakes its host.
/// Storage and callback fit the fixed pool/phase bookkeeping envelopes.
pub struct Task<T>(crate::sys::Shared<crate::sys::Lock<Option<Result<T, ()>>>>);
impl<T> Task<T> {
    pub(crate) fn is_finished(&self) -> bool {
        self.0.lock().is_some()
    }
}
/// One controller pool: the controller runtime's blocking threads, `dc + 1`
/// of them. The host lends its prepared runtime's handle; a library
/// caller's Execution builds and owns one, and shuts it down on drop.
pub struct Execution {
    runtime: Option<tokio::runtime::Runtime>,
    pool: tokio::runtime::Handle,
    /// Tasks submitted and not yet stored: at most `dc + 1`, local copies,
    /// failure writes and phase-6/7 tasks together.
    outstanding: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    capacity: usize,
    dispatch: Option<SyncSender<Dispatch>>,
    ends: Receiver<End>,
    workers: Vec<JoinHandle<()>>,
    end_wake: Arc<Wake>,
    wake: Arc<Wake>,
    spec: Arc<Spec>,
    context: Arc<OnceLock<worker::Context>>,
}
impl Execution {
    /// Prewarm every pool thread before any stage starts. Pool tasks have
    /// explicit 2 MiB stacks; extraction uses `worker_stack` instead.
    pub fn new(controller: &Controller) -> Result<Self, SetupError> {
        let execution = Self::prepare(&controller.prepared)?;
        execution.bind(controller)?;
        Ok(execution)
    }
    /// A library run's Execution: its own controller runtime, whose `dc + 1`
    /// blocking threads start here, before destructive spool cleanup.
    pub fn prepare(p: &crate::phase1::PreparedRun) -> Result<Self, SetupError> {
        let threads = pool_size(&p.spec)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(threads)
            .thread_stack_size(2 << 20)
            // Never retire a prewarmed thread: a respawn would allocate.
            .thread_keep_alive(std::time::Duration::MAX)
            .build()
            .map_err(|_| SetupError::Execution)?;
        prewarm(runtime.handle(), threads).map_err(|_| SetupError::Execution)?;
        let wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let mut execution = Self::prepare_on(p, runtime.handle().clone(), wake)?;
        execution.runtime = Some(runtime);
        Ok(execution)
    }
    /// The host's Execution, on the controller runtime it prepared and
    /// prewarmed (`lifetime::Runtimes`), woken by the lifecycle's stop wake.
    pub fn prepare_on(
        p: &crate::phase1::PreparedRun,
        pool: tokio::runtime::Handle,
        stop_wake: Arc<Wake>,
    ) -> Result<Self, SetupError> {
        let capacity = pool_size(&p.spec)?;
        let end_wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let (send, ends) = mpsc::sync_channel(p.spec.threads as usize);
        // Prime the bounded end channel's lazy receiver wake before workers
        // can finish or disconnect after the running head.
        let _ = ends.recv_timeout(std::time::Duration::from_millis(1));
        let (dispatch, receive) = mpsc::sync_channel(p.spec.threads as usize);
        let receive = Arc::new(Mutex::new(receive));
        let mut execution = Self {
            runtime: None,
            pool,
            outstanding: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            capacity,
            dispatch: Some(dispatch),
            ends,
            workers: Vec::with_capacity(p.spec.threads as usize),
            end_wake,
            wake: stop_wake,
            spec: p.spec.clone(),
            context: Arc::new(OnceLock::new()),
        };
        let context = execution.context.clone();
        let (started, ready) = mpsc::sync_channel(p.spec.threads as usize);
        let gate = StartupGate::new();
        for i in 0..p.spec.threads {
            let receive = receive.clone();
            let send = send.clone();
            let wake = execution.end_wake.clone();
            let context = context.clone();
            let started = started.clone();
            let gate = gate.0.clone();
            let worker = std::thread::Builder::new()
                .name(format!("batch-{i}"))
                .stack_size(worker::worker_stack(
                    &unityrs_core::limits::Limits::default(),
                ))
                .spawn(move || {
                    if let Ok(receiver) = receive.lock() {
                        let _ = receiver.recv_timeout(std::time::Duration::from_millis(1));
                    }
                    let _ = started.try_send(());
                    StartupGate::wait(&gate);
                    drop(gate);
                    worker::run(&context, &receive, &send, &wake);
                })
                .map_err(|_| SetupError::Execution)?;
            execution.workers.push(worker);
        }
        for _ in 0..p.spec.threads {
            ready
                .recv_timeout(std::time::Duration::from_secs(1))
                .map_err(|_| SetupError::Execution)?;
        }
        drop(gate);
        Ok(execution)
    }
    pub fn bind(&self, controller: &Controller) -> Result<(), SetupError> {
        if !self.matches(&controller.prepared.spec) {
            return Err(SetupError::Execution);
        }
        self.context
            .set(worker::Context::new(controller))
            .map_err(|_| SetupError::Execution)
    }
    pub(crate) fn matches(&self, spec: &Arc<Spec>) -> bool {
        Arc::ptr_eq(&self.spec, spec)
    }
    #[must_use]
    pub fn wake(&self) -> Arc<Wake> {
        self.wake.clone()
    }
    /// Completion descriptor, shared with a host integrating its wake set.
    #[must_use]
    pub fn end_wake(&self) -> Arc<Wake> {
        self.end_wake.clone()
    }
    /// The most pool tasks outstanding at once so far (at most `dc + 1`).
    #[must_use]
    pub fn pool_peak(&self) -> usize {
        self.peak.load(Ordering::Acquire)
    }
    /// One task cell on the controller runtime's blocking pool: tokio's
    /// infallible allocation, the single admitted third-party frame (A13).
    /// Every other allocation here is fallible, and the bound is ours.
    pub(crate) fn spawn<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> io::Result<Task<T>> {
        let result = crate::sys::Shared::new(crate::sys::Lock::new(None))?;
        self.outstanding
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.capacity).then_some(n + 1)
            })
            .map(|n| self.peak.fetch_max(n + 1, Ordering::AcqRel))
            .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?;
        let slot = result.clone();
        let outstanding = self.outstanding.clone();
        let wake = self.end_wake.clone();
        let spawned = catch_unwind(AssertUnwindSafe(|| {
            drop(self.pool.spawn_blocking(move || {
                let value = catch_unwind(AssertUnwindSafe(job)).map_err(|_| ());
                // The callback and its captures have dropped; free the place
                // before publishing, so a woken host may submit again at once.
                outstanding.fetch_sub(1, Ordering::AcqRel);
                *slot.lock() = Some(value);
                // Readiness cannot trail wake.
                let _ = wake.notify();
            }));
        }));
        if spawned.is_err() {
            // tokio panics only when it cannot start a thread; the pool was
            // prewarmed, so this is a host failure, never an abort.
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(Task(result))
    }
    #[allow(clippy::unused_self)] // A result is taken through the Execution that ran it.
    pub(crate) fn finish<T>(&self, task: &Task<T>) -> Result<T, ()> {
        // No join/block_on: the readiness flag is the produced result itself.
        task.0.lock().take().ok_or(())?
    }
    pub(crate) fn dispatch(&self, job: Dispatch) -> Result<(), ()> {
        self.context.get().ok_or(())?;
        self.dispatch
            .as_ref()
            .ok_or(())?
            .try_send(job)
            .map_err(|_| ())
    }
    pub(crate) fn receive(&self) -> Result<End, TryRecvError> {
        self.ends.try_recv()
    }
    pub(crate) fn wait(&self) -> io::Result<()> {
        let result = crate::sys::poll_wakes(&self.end_wake, &self.wake);
        // Drain even after interruption/timeout/wake-only. EAGAIN is empty.
        self.end_wake.drain()?;
        self.wake.drain()?;
        result
    }
    /// Explicit test/host quiescence, after all held work has been released.
    /// This can wait; it must never be called on the stop/handover path.
    pub fn quiesce(&mut self) -> Result<(), SetupError> {
        self.dispatch.take();
        for worker in self.workers.drain(..) {
            worker.join().map_err(|_| SetupError::Execution)?;
        }
        // An owned runtime's drop waits for released pool work, including
        // late results still owned by the controller's measured slots.
        drop(self.runtime.take());
        Ok(())
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.dispatch.take();
        // Dropping JoinHandles detaches; held filesystem work retains all
        // owners until it cooperates. Drop never joins: an owned runtime is
        // shut down in the background, its threads ending as their tasks do.
        // A host's lent runtime is not ours, and its owner parks it.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}
fn pool_size(spec: &Spec) -> Result<usize, SetupError> {
    spec.download_concurrency
        .checked_add(1)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(SetupError::Execution)
}
/// Start every blocking thread before the head, then widen tokio's queue to
/// the pool's size while every thread is held, so a later submission neither
/// starts a thread nor grows the queue. Shares one absolute second.
fn prewarm(pool: &tokio::runtime::Handle, threads: usize) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let held = Arc::new((Mutex::new((0_usize, false)), Condvar::new()));
    let ran = Arc::new(AtomicUsize::new(0));
    let wait = |until: &dyn Fn(&(usize, bool)) -> bool| {
        let (lock, changed) = &*held;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        while !until(&state) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            state = changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    };
    let release = || {
        let (lock, changed) = &*held;
        lock.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        changed.notify_all();
    };
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        for _ in 0..threads {
            let held = held.clone();
            drop(pool.spawn_blocking(move || {
                let (lock, changed) = &*held;
                let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
                state.0 += 1;
                changed.notify_all();
                while !state.1 {
                    state = changed.wait(state).unwrap_or_else(PoisonError::into_inner);
                }
            }));
        }
    }));
    let all = spawned.is_ok() && wait(&|s| s.0 == threads);
    let queued = all
        && catch_unwind(AssertUnwindSafe(|| {
            for _ in 0..threads {
                let ran = ran.clone();
                drop(pool.spawn_blocking(move || {
                    ran.fetch_add(1, Ordering::AcqRel);
                }));
            }
        }))
        .is_ok();
    release();
    while queued && ran.load(Ordering::Acquire) != threads {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::yield_now();
    }
    if queued {
        Ok(())
    } else {
        Err(io::ErrorKind::TimedOut.into())
    }
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
