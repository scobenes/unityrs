//! Local library controller over an exclusively prepared spool. B7 owns
//! startup/publication; B9 owns process lifetime, arbitration and diagnostics.
mod commit;
pub(crate) mod failure;
mod passes;
mod progress;
mod queue;
mod receive;
use crate::sys::Shared;
use crate::{
    attempt,
    control::{Control, Culprit},
    execution::Execution,
    job::{End, RunFailure},
    phase1::PreparedRun,
    spool::{Counter, Directory, FileSystem},
    sys::Wake,
};
pub use queue::Admission;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    UrlUnsupported,
    Admission,
    Execution,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    ReadyForWinners,
    HandOver,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    StageDispatched(u32),
    StageReceived(u32),
    PeekDispatched(u32),
    PeekReceived(u32),
    AttemptDispatched(u32),
    AttemptReceived(u32),
    Renamed(u32),
    HandOver,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Stage(u32, crate::stage::local::Point),
    Peek(u32, crate::peek::Point),
    Attempt(u32, attempt::Point),
    BeforeSend(crate::phase1::JobKind, u32),
    EndQueued(crate::phase1::JobKind, u32),
    BeforeCommitLoad(u32),
    AfterRenameTrace(u32),
    FailureCreated(u32),
    BeforePoll,
    PollReturned,
}
/// Typed bounded observations, independent of the B9 trace protocol. Worker
/// callbacks run on their job's thread; commit callbacks run on the controller.
pub trait Hooks: unityrs_export::ngui::Hooks + Send + Sync {
    fn event(&self, _event: Event) {}
    fn point(&self, _point: Point) {}
}
pub struct NoHooks;
impl unityrs_export::ngui::Hooks for NoHooks {}
impl Hooks for NoHooks {}

/// Trusted prepared directories and their common claim ledger. Construction
/// pins the existing root descriptor; it never adopts artifacts or prepares
/// directories.
#[derive(Clone)]
pub struct PreparedSpool {
    pub root: Arc<PathBuf>,
    pub counter: Arc<Counter>,
    pub fs: Arc<dyn FileSystem>,
    held: Arc<std::fs::File>,
    lease: Option<(
        Arc<crate::start::spool::Started>,
        Arc<crate::sink::directory::Directory>,
    )>,
}
impl PreparedSpool {
    pub fn new(
        root: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> std::io::Result<Self> {
        let held = Arc::new(crate::sys::open_directory(&root)?);
        Ok(Self {
            root: Arc::new(root),
            counter,
            fs,
            held,
            lease: None,
        })
    }
    #[must_use]
    pub fn retained(
        mut self,
        spool: Arc<crate::start::spool::Started>,
        sink: Arc<crate::sink::directory::Directory>,
    ) -> Self {
        self.lease = Some((spool, sink));
        self
    }
    pub(crate) fn create_directory(
        &self,
        area: &str,
        input: u32,
    ) -> Result<Directory, crate::spool::Error> {
        Directory::create_anchored(
            self.directory(area, input)?,
            self.held.clone(),
            crate::spool::path::indexed(std::path::Path::new(area), "", u64::from(input))?,
            self.counter.clone(),
            self.fs.clone(),
        )
    }
    pub(crate) fn input(&self) -> Result<PathBuf, crate::spool::Error> {
        crate::spool::path::join(&self.root, std::path::Path::new("in"))
    }
    pub(crate) fn directory(&self, area: &str, input: u32) -> Result<PathBuf, crate::spool::Error> {
        let area = crate::spool::path::join(&self.root, std::path::Path::new(area))?;
        crate::spool::path::indexed(&area, "", u64::from(input))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Halt;
impl From<()> for Halt {
    fn from((): ()) -> Self {
        Self
    }
}

/// These facts survive the drive closure's unwind. No queued end contributes
/// figures until received; a late pool completion never commits itself.
pub struct Controller {
    pub(crate) prepared: PreparedRun,
    pub(crate) spool: PreparedSpool,
    pub(crate) control: Arc<dyn Control>,
    pub(crate) hooks: Arc<dyn Hooks>,
    pub(crate) admission: Admission,
    pub(crate) pending_commit: AtomicU64,
    pub(crate) failure_writer_peak: u64,
    pub(crate) rename_failure: Option<(u32, Option<i32>)>,
    pub(crate) pending_directory: Option<Directory>,
    pub(crate) failure_task:
        Option<crate::execution::Task<Result<failure::FailureEnd, RunFailure>>>,
    pub(crate) failure_owner: Option<Shared<OnceLock<Directory>>>,
    pub(crate) failure_reservation: u64,
    pub(crate) failure: Option<RunFailure>,
    pub(crate) received: Option<End>,
    driven: bool,
    pub(crate) progress: Option<crate::progress::live::Sender>,
}
impl Controller {
    pub fn new(
        mut prepared: PreparedRun,
        spool: PreparedSpool,
        control: Arc<dyn Control>,
        hooks: Arc<dyn Hooks>,
    ) -> Result<Self, SetupError> {
        // Library callers may edit the public prepared spec before setup.
        // Rebuild envelopes here if their caps changed; still before the head.
        if prepared.memory.m != prepared.spec.max_decoded_bytes
            || prepared.memory.d != prepared.spec.dep_cache_bytes
        {
            prepared.memory = Arc::new(
                crate::job_memory::Plan::new(&prepared.spec).map_err(|_| SetupError::Admission)?,
            );
        }
        crate::stage::require_local(&prepared.spec).map_err(|_| SetupError::UrlUnsupported)?;
        let s = &prepared.spec;
        let admission = Admission::new(
            s.threads,
            s.max_decoded_bytes,
            s.dep_cache_bytes,
            s.max_inflight_uncompressed_bytes,
        )?;
        if s.download_concurrency == 0 {
            return Err(SetupError::Admission);
        }
        Ok(Self {
            prepared,
            spool,
            control,
            hooks,
            admission,
            pending_commit: AtomicU64::new(u64::MAX),
            failure_writer_peak: 0,
            rename_failure: None,
            pending_directory: None,
            failure_task: None,
            failure_owner: None,
            failure_reservation: 0,
            failure: None,
            received: None,
            driven: false,
            progress: None,
        })
    }
    pub fn progress(&mut self, sender: crate::progress::live::Sender) {
        self.prepared.memory.observe(&sender.attempts());
        self.progress = Some(sender);
    }
    /// Consume only after the normal boundary and after all receipts.
    pub fn into_state(self) -> PreparedRun {
        self.prepared
    }
    #[must_use]
    pub fn state(&self) -> &PreparedRun {
        &self.prepared
    }
    #[must_use]
    pub fn pending_commit(&self) -> Option<u32> {
        u32::try_from(self.pending_commit.load(Ordering::Acquire)).ok()
    }
    #[must_use]
    pub fn rename_failure(&self) -> Option<(u32, Option<i32>)> {
        self.rename_failure
    }
    #[must_use]
    pub fn failure(&self) -> Option<RunFailure> {
        self.failure
    }
    #[must_use]
    pub fn failure_writer_peak(&self) -> u64 {
        self.failure_writer_peak
    }
    #[must_use]
    pub fn reserved_slots(&self) -> usize {
        self.prepared
            .active
            .iter()
            .filter(|s| s.job.is_some())
            .count()
    }
    /// Execution must outlive this call and any handover. This entry is once
    /// only: a boundary is frozen, and later messages remain unreceived.
    pub fn drive_local(&mut self, execution: &mut Execution) -> Boundary {
        if self.driven || !execution.matches(&self.prepared.spec) {
            self.fail(RunFailure::new(attempt::ReasonCode::Internal));
            return Boundary::HandOver;
        }
        self.driven = true;
        let result = catch_unwind(AssertUnwindSafe(|| self.drive(execution)));
        let boundary = match result {
            Ok(Ok(())) if !self.control.stopped() => Boundary::ReadyForWinners,
            Ok(Ok(()) | Err(Halt)) => {
                if !self.control.stopped() {
                    self.internal();
                }
                Boundary::HandOver
            }
            Err(_) => {
                self.fail(RunFailure::new(attempt::ReasonCode::Internal));
                Boundary::HandOver
            }
        };
        if boundary == Boundary::ReadyForWinners {
            self.sample_now();
        }
        if boundary == Boundary::HandOver {
            // An observer fault cannot prevent the host from taking frozen state.
            let _ = catch_unwind(AssertUnwindSafe(|| self.hooks.event(Event::HandOver)));
        }
        boundary
    }
    fn drive(&mut self, execution: &mut Execution) -> Result<(), Halt> {
        self.check()?;
        if let Some(p) = &self.progress {
            p.update(|s| s.phase = crate::progress::Phase::Stage);
        }
        self.stage(execution)?;
        self.stage_barrier(execution)?;
        if let Some(p) = &self.progress {
            p.update(|s| s.phase = crate::progress::Phase::Extract);
        }
        self.peek_queue()?;
        self.jobs(execution, true)?;
        self.peek_barrier(execution)?;
        self.jobs(execution, false)?;
        self.check()
    }
    pub(crate) fn check(&self) -> Result<(), Halt> {
        self.sample();
        if self.control.stopped() {
            Err(Halt)
        } else {
            Ok(())
        }
    }
    pub(crate) fn fail(&mut self, failure: RunFailure) -> Halt {
        RunFailure::reduce(&mut self.failure, failure);
        self.control.request_stop(failure, Culprit::None);
        Halt
    }
    pub(crate) fn internal(&mut self) -> Halt {
        self.fail(RunFailure::new(attempt::ReasonCode::Internal))
    }
    pub(crate) fn io(&mut self, error: crate::spool::Error) -> Halt {
        self.fail(error.into())
    }
    pub(crate) fn wait(&mut self, execution: &mut Execution) -> Result<(), Halt> {
        self.hooks.point(Point::BeforePoll);
        execution
            .wait()
            .map_err(|_| self.fail(RunFailure::new(attempt::ReasonCode::HostIo)))?;
        self.hooks.point(Point::PollReturned);
        // W1: drain after every poll cause, before testing stop.
        self.drain(execution)
    }
    #[must_use]
    pub fn wake(execution: &Execution) -> Arc<Wake> {
        execution.wake()
    }
}
