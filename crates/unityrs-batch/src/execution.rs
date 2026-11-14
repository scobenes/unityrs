//! Explicit execution ownership, outside `Controller::drive_local`'s unwind
//! boundary. Drop never joins: the B9 host parks this owner after handover;
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
        Arc, Condvar, Mutex, OnceLock,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::JoinHandle,
};
type Job = Box<dyn FnOnce() + Send>;
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
pub struct Execution {
    pool_workers: Vec<JoinHandle<()>>,
    dispatch: Option<SyncSender<Dispatch>>,
    pool_jobs: Option<SyncSender<Job>>,
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
    /// Spawn idle workers and prewarm the pool before destructive spool cleanup.
    pub fn prepare(p: &crate::phase1::PreparedRun) -> Result<Self, SetupError> {
        let pool = p
            .spec
            .download_concurrency
            .checked_add(1)
            .ok_or(SetupError::Execution)? as usize;
        let end_wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let (send, ends) = mpsc::sync_channel(p.spec.threads as usize);
        // Prime the bounded end channel's lazy receiver wake before workers
        // can finish or disconnect after the running head.
        let _ = ends.recv_timeout(std::time::Duration::from_millis(1));
        let (dispatch, receive) = mpsc::sync_channel(p.spec.threads as usize);
        let receive = Arc::new(Mutex::new(receive));
        let (pool_send, pool_receive) = mpsc::sync_channel(pool);
        let mut execution = Self {
            pool_workers: Vec::with_capacity(pool),
            dispatch: Some(dispatch),
            pool_jobs: Some(pool_send),
            ends,
            workers: Vec::with_capacity(p.spec.threads as usize),
            end_wake,
            wake,
            spec: p.spec.clone(),
            context: Arc::new(OnceLock::new()),
        };
        execution
            .prewarm(pool, pool_receive)
            .map_err(|_| SetupError::Execution)?;
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
    fn prewarm(&mut self, pool: usize, receive: Receiver<Job>) -> io::Result<()> {
        // Start every blocking drive before the head. Later jobs use this
        // bounded channel, without per-job runtime/task allocations.
        let (send, ready) = mpsc::sync_channel(pool);
        let receive = Arc::new(Mutex::new(receive));
        let gate = StartupGate::new();
        for index in 0..pool {
            let send = send.clone();
            let receive = receive.clone();
            let wake = self.end_wake.clone();
            let gate = gate.0.clone();
            let worker = std::thread::Builder::new()
                .name(format!("batch-pool-{index}"))
                .stack_size(2 << 20)
                .spawn(move || {
                    // std's bounded channel lazily initializes a thread-local
                    // wait context. Force that setup before reporting readiness.
                    if let Ok(receiver) = receive.lock() {
                        let _ = receiver.recv_timeout(std::time::Duration::from_millis(1));
                    }
                    let _ = send.try_send(());
                    StartupGate::wait(&gate);
                    drop(gate);
                    loop {
                        let job = match receive.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => break,
                        };
                        let Ok(job) = job else { break };
                        job();
                        // The callback and its captures have dropped, and its
                        // result is already stored. Readiness cannot trail wake.
                        let _ = wake.notify();
                    }
                })?;
            self.pool_workers.push(worker);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let ready = (0..pool).all(|_| {
            ready
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .is_ok()
        });
        drop(gate);
        if ready {
            Ok(())
        } else {
            Err(io::ErrorKind::TimedOut.into())
        }
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
    pub(crate) fn spawn<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> io::Result<Task<T>> {
        let result = crate::sys::Shared::new(crate::sys::Lock::new(None))?;
        let slot = result.clone();
        let callback: Job = crate::alloc::try_box(move || {
            let result = catch_unwind(AssertUnwindSafe(job)).map_err(|_| ());
            *slot.lock() = Some(result);
        })?;
        self.pool_jobs
            .as_ref()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .try_send(callback)
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(Task(result))
    }
    pub(crate) fn finish<T>(&self, task: &Task<T>) -> Result<T, ()> {
        self.pool_jobs.as_ref().ok_or(())?;
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
        self.pool_jobs.take();
        // Explicit shutdown waits for released pool work, including late
        // results still owned by the controller's measured slots.
        for worker in self.pool_workers.drain(..) {
            worker.join().map_err(|_| SetupError::Execution)?;
        }
        Ok(())
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.dispatch.take();
        self.pool_jobs.take();
        // Dropping JoinHandles detaches; held filesystem work retains all
        // owners until it cooperates. Drop must never join on handover.
    }
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
