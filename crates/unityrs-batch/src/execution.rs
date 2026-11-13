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
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::JoinHandle,
};
use tokio::{
    runtime::{Builder, Runtime},
    task::JoinHandle as PoolTask,
};

pub struct Execution {
    runtime: Option<Runtime>,
    dispatch: Option<SyncSender<Dispatch>>,
    ends: Receiver<End>,
    workers: Vec<JoinHandle<()>>,
    end_wake: Arc<Wake>,
    wake: Arc<Wake>,
    spec: Arc<Spec>,
}
impl Execution {
    /// Prewarm every pool thread before any stage starts. Pool tasks have
    /// explicit 2 MiB stacks; extraction uses `worker_stack` instead.
    pub fn new(controller: &Controller) -> Result<Self, SetupError> {
        let p = &controller.prepared;
        let pool = p
            .spec
            .download_concurrency
            .checked_add(1)
            .ok_or(SetupError::Execution)? as usize;
        let runtime = Builder::new_current_thread()
            .max_blocking_threads(pool)
            .thread_stack_size(2 << 20)
            .thread_keep_alive(std::time::Duration::from_hours(24))
            .build()
            .map_err(|_| SetupError::Execution)?;
        let end_wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let wake = Arc::new(Wake::new().map_err(|_| SetupError::Execution)?);
        let (send, ends) = mpsc::sync_channel(p.spec.threads as usize);
        let (dispatch, receive) = mpsc::sync_channel(p.spec.threads as usize);
        let receive = Arc::new(Mutex::new(receive));
        let mut execution = Self {
            runtime: Some(runtime),
            dispatch: Some(dispatch),
            ends,
            workers: Vec::with_capacity(p.spec.threads as usize),
            end_wake,
            wake,
            spec: p.spec.clone(),
        };
        execution.prewarm(pool).map_err(|_| SetupError::Execution)?;
        let context = Arc::new(worker::Context::new(controller));
        for i in 0..p.spec.threads {
            let receive = receive.clone();
            let send = send.clone();
            let wake = execution.end_wake.clone();
            let context = context.clone();
            let worker = std::thread::Builder::new()
                .name(format!("batch-{i}"))
                .stack_size(worker::worker_stack(
                    &unityrs_core::limits::Limits::default(),
                ))
                .spawn(move || worker::run(&context, &receive, &send, &wake))
                .map_err(|_| SetupError::Execution)?;
            execution.workers.push(worker);
        }
        Ok(execution)
    }
    fn prewarm(&self, pool: usize) -> io::Result<()> {
        // Startup-only W20: tasks are held until all thread spawns have been
        // observed, with a timeout and an unconditional release on failure.
        let (send, receive) = mpsc::sync_channel(pool);
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| io::Error::other("runtime unavailable"))?;
        for _ in 0..pool {
            let send = send.clone();
            let gate = gate.clone();
            runtime.spawn_blocking(move || {
                let _ = send.try_send(());
                let (lock, changed) = &*gate;
                if let Ok(open) = lock.lock() {
                    let _ = changed.wait_timeout_while(
                        open,
                        std::time::Duration::from_secs(1),
                        |open| !*open,
                    );
                }
            });
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let ready = (0..pool).all(|_| {
            receive
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .is_ok()
        });
        let (lock, changed) = &*gate;
        if let Ok(mut open) = lock.lock() {
            *open = true;
            changed.notify_all();
        }
        if ready {
            Ok(())
        } else {
            Err(io::Error::other("pool startup timeout"))
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
    ) -> Result<PoolTask<T>, ()> {
        self.runtime
            .as_ref()
            .map(|rt| rt.spawn_blocking(job))
            .ok_or(())
    }
    pub(crate) fn finish<T>(&self, task: PoolTask<T>) -> Result<T, ()> {
        // W2: called only after is_finished, never while the filesystem can
        // hold a pool task. All wait turns poll the host's stop wake instead.
        self.runtime
            .as_ref()
            .ok_or(())?
            .block_on(task)
            .map_err(|_| ())
    }
    pub(crate) fn dispatch(&self, job: Dispatch) -> Result<(), ()> {
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
        let runtime = self.runtime.take().ok_or(SetupError::Execution)?;
        // Explicit shutdown waits for released pool tasks, including late
        // results still owned by the controller's measured slots.
        drop(runtime);
        Ok(())
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.dispatch.take();
        // Dropping JoinHandles detaches. Runtime's ordinary Drop would wait
        // for held blocking work, so only its non-waiting shutdown is allowed.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}
