//! The process host (`docs/plans/batch.md` 2.11): the binary's run, from
//! step 1's services to `exit_now`. [`Startup::prepare`] is step 1, before
//! phase 1; [`run`] is steps 3 to 8, and the threads it spawns end the
//! process. Everything the threads share is leaked, the runtimes and the
//! Execution among them: it lives until `_exit`, and no unwind drops it.
//!
//! Threads: the progress thread; the finisher, which waits on its wake
//! for the hand-over, finishes and makes the exit; the lifecycle thread
//! ([`Watch`]), which owns `T_hard`; the stdin watch's own thread under
//! `stop_on_stdin_eof`; the workers; and the controller, last, which runs
//! the body ([`super::Body`]), hands over and parks with the owners. Each
//! body runs inside `catch_unwind` (2.11's *Panics*), the main thread's
//! too. The body builds its hand-over in [`super::Parked`], so a panic
//! after the `running` head still hands over with the facts: between
//! `running` and the facts' write lie only moves, the controller's own
//! drive catching every unwind.
mod early;
mod network;
mod threads;
#[cfg(feature = "test-hooks")]
use super::hooks::{Log, Point, Points, TestHooks};
use super::{Error, Lifetime, Options};
use crate::{
    diagnostic::{self, Secrets, Writer},
    execution::Execution,
    finish::HandOver,
    lifetime::{Policy, Process, Runtimes, StdinWatch, Stop, TimingError, Watch},
    phase::Account,
    phase1::PreparedRun,
    progress::{self, Phase, Snapshot, clock::Clock, live::Service},
    reason::{Code, StopReason},
    sys::{self, RegisteredWake, Wake},
};
use early::Early;
use std::{
    fs::File,
    os::fd::AsFd,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU8, Ordering::SeqCst},
    },
    time::Duration,
};
pub use threads::exit_of;

/// A thread step 7 spawns (the `spawn` hook's names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thread {
    Progress,
    Finisher,
    Lifecycle,
    Worker,
    Controller,
}
/// Step 1's services, prepared before phase 1.
pub struct Startup {
    writer: Writer,
    /// Two spare descriptors for the main thread's early lines.
    spares: [Option<std::os::fd::OwnedFd>; 2],
    #[cfg(feature = "test-hooks")]
    hooks: TestHooks,
    #[cfg(feature = "test-hooks")]
    lifecycle: Option<File>,
    #[cfg(feature = "test-hooks")]
    log: Option<Arc<Log>>,
}
impl Startup {
    /// Step 1, after the job-control signals' ignore (the caller's): the
    /// panic hook, the test hooks, the stderr writer, the lifecycle trace
    /// FIFO, and the five handled signals blocked. `Err` is 69
    /// `startup_failed`, its line the caller's to write.
    pub fn prepare() -> Result<Self, Error> {
        // First, so a full descriptor table leaves room for the lines of a
        // later 69 (`EMFILE`); a failure here drops them before its own.
        let spares = early::spares();
        diagnostic::panic::install();
        #[cfg(feature = "test-hooks")]
        let hooks = TestHooks::from_variables().map_err(|r| Error::startup(r.0, None))?;
        let writer = Writer::prepare().map_err(|e| Error::startup("stderr", e.raw_os_error()))?;
        #[cfg(feature = "test-hooks")]
        let (lifecycle, log) = {
            let lifecycle = hooks
                .trace
                .as_ref()
                .map(|trace| {
                    let mut path = trace.clone().into_os_string();
                    path.push(".lifecycle");
                    sys::lifecycle::open_trace(path.as_ref())
                        .map_err(|(call, errno)| Error::startup(call, errno))
                })
                .transpose()?;
            let log = hooks
                .trace_log()
                .map_err(|e| Error::startup("open", e.raw_os_error()))?;
            if let Some((_, action)) = hooks.point.iter().find(|(p, _)| *p == Point::ExitNow) {
                let wait = match action {
                    super::hooks::Action::Wait(path) => Some(
                        std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
                            .map_err(|_| Error::startup("UNITYRS_TEST_POINT", None))?,
                    ),
                    _ => None,
                };
                sys::lifecycle::held::arm(wait, log.as_ref().map(|l| l.fd()));
            }
            (lifecycle, log)
        };
        sys::signals::block().map_err(|e| Error::startup("signal", e.raw_os_error()))?;
        Ok(Self {
            writer,
            spares,
            #[cfg(feature = "test-hooks")]
            hooks,
            #[cfg(feature = "test-hooks")]
            lifecycle,
            #[cfg(feature = "test-hooks")]
            log,
        })
    }
    /// The test hooks step 1 read, for a library-API child to amend.
    #[cfg(feature = "test-hooks")]
    pub fn hooks(&mut self) -> &mut TestHooks {
        &mut self.hooks
    }
    /// The spare descriptors closed for phase 1, whose `JOB.json` open
    /// they would otherwise take at a low `RLIMIT_NOFILE`.
    pub fn release_spares(&mut self) {
        self.spares = [None, None];
    }
    /// The spares opened again once phase 1 has read and validated the spec.
    pub fn restore_spares(&mut self) {
        self.spares = early::spares();
    }
}
/// What every thread shares, leaked at step 4.
struct Host {
    process: Process,
    network: OnceLock<network::Resources>,
    spec: Arc<crate::spec::model::Spec>,
    values: OnceLock<Arc<crate::stage::env::Values>>,
    masked: OnceLock<Vec<&'static str>>,
    mask_buffer: Mutex<Option<Vec<&'static str>>>,
    writer: &'static Writer,
    /// The controller's and the finisher's wakes.
    ctrl: Arc<Wake>,
    fin: Arc<Wake>,
    /// The hand-over, set once by the controller.
    slot: Mutex<Option<Result<HandOver, Error>>>,
    stdin_ended: AtomicBool,
    credentials: OnceLock<[&'static str; 3]>,
    /// The finisher's code so far (0 for none), outside its unwind.
    finishing: AtomicU8,
    early: Early,
    bound: u64,
    #[cfg(feature = "test-hooks")]
    log: Option<Arc<Log>>,
    #[cfg(feature = "test-hooks")]
    handover: Option<std::path::PathBuf>,
}
/// What step 7 adds, leaked before the first thread that reads it.
struct Running {
    service: Service,
    policy: Policy,
    fin: Account,
    started_at: [u8; 20],
    #[cfg(feature = "test-hooks")]
    points: Arc<Points>,
}
/// The main thread's owners, static, so a panic's unwind there drops none
/// of them (2.11's *Panics*).
struct Held {
    spec: OnceLock<Arc<crate::spec::model::Spec>>,
    known: OnceLock<Vec<&'static str>>,
    /// The stderr writer, the main thread's first owner.
    writer: OnceLock<Writer>,
    host: OnceLock<&'static Host>,
    runtimes: OnceLock<Runtimes>,
    /// The workers' Execution, taken by the controller at its start.
    execution: Mutex<Option<Execution>>,
}
impl Host {
    fn secrets(&'static self) -> Secrets<'static> {
        Secrets {
            values: self
                .masked
                .get()
                .or_else(|| HELD.known.get())
                .map_or(&[], Vec::as_slice),
            credentials: &self.credentials,
        }
    }
    fn request(&self, reason: StopReason) {
        self.process.request(Stop::external(reason));
    }
    #[cfg(feature = "test-hooks")]
    fn trace(&self, args: std::fmt::Arguments<'_>) {
        if let Some(log) = &self.log {
            log.line(args);
        }
    }
    fn finishing(&self, code: Code) {
        self.finishing.store(code.value(), SeqCst);
    }
    /// The finisher's code so far, else the end code (the final code once
    /// stored, else the stop word's).
    fn code_so_far(&self) -> Code {
        let end = self.process.state.end_code(None);
        Code::from_value(self.finishing.load(SeqCst)).map_or(end, |own| own.worst(end))
    }
    /// A 69 once the process word exists: the final code first, so a second
    /// signal exits 69, then the main thread's lines (2.11).
    fn fail(&'static self, error: &Error) -> ! {
        self.process.state.store_final(error.code);
        self.early.write_masked(error, Some(&self.secrets()));
        self.process.exit(error.code)
    }
    /// A 69 once the progress thread runs: its mailbox, at most 1 s.
    fn hand(&self, running: &Running, error: Error) -> ! {
        self.process.state.store_final(error.code);
        let code = error.code;
        let _ = running.service.exit(
            self.snapshot(running),
            exit_of(Err(error)),
            Duration::from_secs(1),
        );
        self.process.exit(code)
    }
    fn snapshot(&self, running: &Running) -> Snapshot {
        let mut snapshot = running
            .service
            .sender()
            .snapshot()
            .unwrap_or_else(|| Snapshot::validate(0));
        snapshot.ms = self.early.clock.ms();
        snapshot
    }
}
/// The 2 MiB stack every host thread takes (2.11's *Threads*).
const STACK: usize = 2 << 20;
fn spawn(
    name: &str,
    which: Option<Thread>,
    failing: Option<Thread>,
    body: impl FnOnce() + Send + 'static,
) -> Result<(), Error> {
    if which.is_some() && failing == which {
        return Err(Error::spawn());
    }
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(STACK)
        .spawn(body)
        .map(drop)
        .map_err(|_| Error::spawn())
}
/// Steps 3 to 8, `note` the uncertified bound's stderr line written first
/// as the main thread's lines are. Never returns: the finisher, the
/// lifecycle thread, a second signal or a startup failure ends the
/// process; a panic of the main thread is `exit_now` at once, with the
/// more severe of 70 and the code stored.
pub fn run(
    startup: Startup,
    clock: Clock,
    prepared: PreparedRun,
    bound: u64,
    note: Option<&str>,
) -> ! {
    diagnostic::format::source(current_secrets);
    let held = &HELD;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        steps(held, startup, clock, prepared, (bound, note));
    }));
    // Its line, written at once by the nonblocking, drop-only route.
    diagnostic::pending::panic(format_args!(""));
    if let Some(host) = held.host.get() {
        host.pending(true);
        host.process
            .exit(Code::Internal.worst(host.process.state.end_code(None)))
    }
    // Before the host: through the writer, with no credentials yet,
    // waiting for another drain no longer than a line may (1 s).
    if let Some(writer) = held.writer.get() {
        let secrets = current_secrets();
        let until = std::time::Instant::now() + Duration::from_secs(1);
        threads::write_pending(writer, &secrets, true, Some(until), || {});
    }
    sys::lifecycle::exit_unprepared(Code::Internal)
}
/// The head's warnings, sampled as it is encoded (7.3): every stderr line
/// dropped (the writer's, the pending ring's unwritten, the early lines'),
/// and the stdin watch's end.
fn warnings() -> crate::finish::Warnings {
    let Some(host) = HELD.host.get() else {
        return crate::finish::Warnings::default();
    };
    crate::finish::Warnings {
        stderr_dropped: host
            .writer
            .dropped()
            .saturating_add(diagnostic::pending::unwritten())
            .saturating_add(progress::early::dropped()),
        stdin_watch_ended: host.stdin_ended.load(std::sync::atomic::Ordering::SeqCst),
        sample: None,
    }
}
/// The warnings both of the host's heads take, the `running` head's and
/// the final one's: [`warnings`], sampled as each is encoded.
const SAMPLED: crate::finish::Warnings = crate::finish::Warnings {
    stderr_dropped: 0,
    stdin_watch_ended: false,
    sample: Some(warnings),
};
/// The spec-only secrets remain available before host construction, including
/// while unwinding out of its setup. Resolved values join through the host.
fn current_secrets() -> Secrets<'static> {
    HELD.host.get().map_or_else(
        || Secrets {
            values: HELD.known.get().map_or(&[], Vec::as_slice),
            credentials: &NO_CREDENTIALS,
        },
        |host| host.secrets(),
    )
}
/// No credentials before the host exists.
static NO_CREDENTIALS: OnceLock<[&'static str; 3]> = OnceLock::new();
static HELD: Held = Held {
    spec: OnceLock::new(),
    known: OnceLock::new(),
    writer: OnceLock::new(),
    host: OnceLock::new(),
    runtimes: OnceLock::new(),
    execution: Mutex::new(None),
};
/// The lifecycle thread's panic: the queued lines, its own among them,
/// before its exit.
fn lifecycle_panicked() {
    if let Some(host) = HELD.host.get() {
        host.pending(true);
    }
}
/// The tally hook's thread registration (a no-op without one).
#[cfg(feature = "test-hooks")]
fn registration(hooks: &TestHooks) -> (fn(), fn()) {
    hooks
        .tally
        .map_or((|| {}, || {}), |tally| (tally.register, tally.mark))
}
#[allow(clippy::too_many_lines)] // 2.11's steps in their order, in one place.
fn steps(
    held: &'static Held,
    startup: Startup,
    clock: Clock,
    mut prepared: PreparedRun,
    (bound, note): (u64, Option<&str>),
) -> ! {
    let spec = held.spec.get_or_init(|| prepared.spec.clone());
    let [mut known, resolved] = std::mem::take(&mut prepared.masker);
    let deadline = spec
        .deadline
        .as_ref()
        .and_then(|d| crate::lifetime::unix_ns(&d.at).ok());
    let writer: &'static Writer = held.writer.get_or_init(|| startup.writer);
    let early = Early::new(clock, deadline, startup.spares);
    #[cfg(feature = "test-hooks")]
    let mut hooks = startup.hooks;
    #[cfg(feature = "test-hooks")]
    let failing = hooks.spawn;
    #[cfg(not(feature = "test-hooks"))]
    let failing: Option<Thread> = None;
    #[cfg(feature = "test-hooks")]
    let (register, mark) = registration(&hooks);
    #[cfg(not(feature = "test-hooks"))]
    let (register, mark): (fn(), fn()) = (|| {}, || {});
    if crate::stage::env::Values::unresolved_secrets(spec, &mut known).is_err() {
        early.fail(&Error::memory());
    }
    let _ = held.known.set(known);
    // The main thread and the early writer thread register too.
    register();
    progress::early::on_thread_start(register);
    if let Some(note) = note {
        early.note(note);
    }
    // Step 3: E57's allocator.
    if sys::allocator(spec.threads).is_err() {
        early.fail(&Error::startup("mallopt", None));
    }
    // Step 4.
    let process = match Process::prepare() {
        Ok(process) => process,
        Err(e) => early.fail(&Error::startup("pipe", e.raw_os_error())),
    };
    let runtimes = match Runtimes::prepare_with(spec.download_concurrency, register) {
        Ok(runtimes) => held.runtimes.get_or_init(|| runtimes),
        Err(e) => early.fail(&Error::startup("runtime", e.raw_os_error())),
    };
    let wake = || Wake::new().map(Arc::new);
    let (ctrl, fin) = match (wake(), wake()) {
        (Ok(ctrl), Ok(fin)) => (ctrl, fin),
        (Err(e), _) | (_, Err(e)) => early.fail(&Error::startup("eventfd", e.raw_os_error())),
    };
    let registered = match RegisteredWake::new(&fin, runtimes.finisher.handle()) {
        Ok(registered) => registered,
        Err(e) => early.fail(&Error::startup("epoll_ctl", e.raw_os_error())),
    };
    let host: &'static Host = Box::leak(Box::new(Host {
        process,
        network: OnceLock::new(),
        spec: spec.clone(),
        values: OnceLock::new(),
        masked: OnceLock::new(),
        mask_buffer: Mutex::new(Some(resolved)),
        writer,
        ctrl,
        fin,
        slot: Mutex::new(None),
        stdin_ended: AtomicBool::new(false),
        credentials: OnceLock::new(),
        finishing: AtomicU8::new(0),
        early,
        bound,
        #[cfg(feature = "test-hooks")]
        log: startup.log,
        #[cfg(feature = "test-hooks")]
        handover: hooks.handover.clone(),
    }));
    let _ = held.host.set(host);
    let (network, permits) = match network::Resources::prepare(
        runtimes,
        &host.process,
        spec.download_concurrency,
        #[cfg(feature = "test-hooks")]
        &mut hooks,
        #[cfg(feature = "test-hooks")]
        host.log.clone(),
    ) {
        Ok(resources) => resources,
        Err(error) => host.fail(&error),
    };
    let _ = host.network.set(network);
    let mut stdin = if spec.stop_on_stdin_eof {
        match StdinWatch::prepare(
            &host.process,
            &host.stdin_ended,
            host.writer,
            host.secrets(),
        ) {
            Ok(watch) => Some(watch),
            Err(_) => host.fail(&Error::startup("stdin", None)),
        }
    } else {
        None
    };
    #[cfg(feature = "test-hooks")]
    let installed = if hooks.init {
        sys::signals::install_as_init(&host.process)
    } else {
        sys::signals::install(&host.process)
    };
    #[cfg(not(feature = "test-hooks"))]
    let installed = sys::signals::install(&host.process);
    if let Err(e) = installed {
        host.fail(&Error::startup("sigaction", e.raw_os_error()));
    }
    // Step 5.
    if let Err(e) = sys::signals::parent(&host.process, spec.parent_pid) {
        host.fail(&Error::startup("prctl", e.raw_os_error()));
    }
    // Step 6: the deadline, then the host checks.
    let grace = u64::from(spec.shutdown_grace_ms);
    let at = spec.deadline.as_ref().map(|d| &d.at);
    #[cfg(feature = "test-hooks")]
    let policy = match (hooks.cutoff_in, deadline, sys::boottime()) {
        // The wall clock read as the cutoff less `cutoff_in`.
        (Some(ms), Some(deadline), Some(boot)) if hooks.cutoff_arm.is_none() => Policy::at(
            at,
            grace,
            deadline - i128::from(grace + ms) * 1_000_000,
            boot,
        ),
        _ => Policy::new(at, grace),
    };
    #[cfg(not(feature = "test-hooks"))]
    let policy = Policy::new(at, grace);
    let policy = match policy {
        Ok(policy) => policy,
        Err(TimingError::DeadlineTooClose) => {
            host.fail(&Error::could_not_start("deadline_too_close"))
        }
        Err(_) => host.fail(&Error::startup("clock_gettime", None)),
    };
    host.early.converted(policy.deadline);
    let mut watch = match Watch::prepare(&host.process, policy, &host.ctrl, &host.fin) {
        Ok(watch) => watch,
        Err(e) => host.fail(&Error::startup("timerfd_create", e.raw_os_error())),
    };
    #[cfg(feature = "test-hooks")]
    if let Some(file) = startup.lifecycle {
        watch.with_trace(file);
    }
    // A version 2 spec's UnityCN key, from the environment, as a header's
    // variable is read (`BATCH_SPEC.md` 1.1): before the host checks (2.7).
    if let Err(error) = super::unitycn_key(&mut prepared, |name| std::env::var_os(name)) {
        host.fail(&error);
    }
    if let Err(error) = host.headers() {
        host.fail(&error);
    }
    if let Some(stdin) = &mut stdin {
        stdin.secrets(host.secrets());
    }
    #[cfg(feature = "test-hooks")]
    let checked = if hooks.skip_host_checks {
        Ok(())
    } else if let Some(root) = &hooks.host_root {
        super::host_check(&prepared, bound, root.as_ref())
    } else {
        super::host_check(&prepared, bound, &crate::start::host::System)
    };
    #[cfg(not(feature = "test-hooks"))]
    let checked = super::host_check(&prepared, bound, &crate::start::host::System);
    if let Err(error) = checked {
        host.fail(&error);
    }
    // Step 7: the progress thread first.
    let file = match std::io::stdout().as_fd().try_clone_to_owned() {
        Ok(fd) => File::from(fd),
        Err(e) => host.fail(&Error::startup("dup", e.raw_os_error())),
    };
    let mut snapshot = Snapshot::validate(host.early.clock.ms());
    snapshot.phase = Phase::Preflight;
    snapshot.inputs.total = prepared.inputs.len() as u64;
    snapshot.inputs.queued = snapshot.inputs.total;
    snapshot.memory.inflight_limit = spec.max_inflight_uncompressed_bytes;
    snapshot.spool.size = spec.spool.size_bytes;
    #[cfg(feature = "test-hooks")]
    let points = Arc::new(Points::new(&hooks, spec.clone(), host.log.clone()));
    #[cfg(feature = "test-hooks")]
    let initializing = {
        let points = points.clone();
        move || {
            register();
            points.at(&Point::Progress);
        }
    };
    #[cfg(not(feature = "test-hooks"))]
    let initializing = register;
    let service = if failing == Some(Thread::Progress) {
        Err(Error::spawn())
    } else {
        Service::start_hosted(file, snapshot, initializing, move || {
            host.request(StopReason::Internal);
        })
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::OutOfMemory {
                Error::memory()
            } else {
                Error::spawn()
            }
        })
    };
    let service = match service {
        Ok(service) => service,
        Err(error) => host.fail(&error),
    };
    // The progress thread writes every later line: the spares are free.
    host.early.release();
    let started_at = sys::utc_now().unwrap_or(*b"1970-01-01T00:00:00Z");
    let fin = Account::observed(crate::bound::C_FIN, Some(service.sender().phase()));
    let running: &'static Running = Box::leak(Box::new(Running {
        service,
        policy,
        fin,
        started_at,
        #[cfg(feature = "test-hooks")]
        points,
    }));
    // Both runtimes' blocking pools, spawned whole.
    #[cfg(feature = "test-hooks")]
    let prewarmed = runtimes.prewarm_short(hooks.start_count);
    #[cfg(not(feature = "test-hooks"))]
    let prewarmed = runtimes.prewarm();
    if prewarmed.is_err() {
        host.hand(running, Error::spawn());
    }
    let finisher_runtime = &runtimes.finisher;
    let spawned = spawn(
        "batch-finisher",
        Some(Thread::Finisher),
        failing,
        move || {
            register();
            threads::finisher_thread(host, running, finisher_runtime, &registered)
        },
    );
    if let Err(error) = spawned {
        host.hand(running, error);
    }
    watch.on_panic(lifecycle_panicked);
    #[cfg(feature = "test-hooks")]
    if let Some(path) = &hooks.cutoff_arm {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) if file.metadata().is_ok_and(|m| m.file_type().is_fifo()) => file,
            _ => host.fail(&Error::startup("UNITYRS_TEST_CUTOFF_ARM", None)),
        };
        watch.cutoff_after(file, hooks.cutoff_in.unwrap_or(0));
    }
    #[cfg(feature = "test-hooks")]
    {
        super::hooks::arm_lifecycle(hooks.tally, running.points.clone());
        watch.mark(super::hooks::lifecycle_mark);
    }
    let spawned = spawn(
        "batch-lifecycle",
        Some(Thread::Lifecycle),
        failing,
        move || watch.run(),
    );
    if let Err(error) = spawned {
        host.hand(running, error);
    }
    if let Some(stdin) = stdin {
        #[cfg(feature = "test-hooks")]
        let points = running.points.clone();
        let spawned = spawn("batch-stdin", None, failing, move || {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                register();
                mark();
                #[cfg(feature = "test-hooks")]
                points.at(&Point::Stdin);
                stdin.run();
            }));
            if ran.is_err() {
                diagnostic::pending::panic(format_args!("the stdin watch: "));
                host.request(StopReason::Internal);
            }
        });
        if let Err(error) = spawned {
            host.hand(running, error);
        }
    }
    let _ = mark;
    // The workers, then the controller, last.
    if failing == Some(Thread::Worker) {
        host.hand(running, Error::spawn());
    }
    let Ok(execution) =
        Execution::prepare_lent(&prepared, &runtimes.controller, host.ctrl.clone(), register)
    else {
        host.hand(running, Error::spawn())
    };
    let staging = if spec.bundles().any(|b| b.url().is_some()) {
        match host.staging(&execution, &prepared, policy, permits) {
            Ok(staging) => Some(staging),
            Err(error) => host.hand(running, error),
        }
    } else {
        None
    };
    *held
        .execution
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(execution);
    #[cfg(feature = "test-hooks")]
    let (options, files) = (
        Options {
            lifetime: Arc::new(Lifetime::process(&host.process)),
            controller_hooks: running.points.clone(),
            publish_hooks: running.points.clone(),
            observer: running.points.clone(),
            parked_drops: None,
            spool: hooks
                .files
                .as_ref()
                .map(|f| f.started.clone() as Arc<dyn crate::spool::FileSystem>),
            network: None,
            warnings: SAMPLED,
        },
        hooks
            .files
            .as_ref()
            .map(|f| (f.spool.clone(), f.sink.clone())),
    );
    #[cfg(not(feature = "test-hooks"))]
    let (options, files): (Options, Option<threads::Files>) = (
        Options {
            lifetime: Arc::new(Lifetime::process(&host.process)),
            warnings: SAMPLED,
            ..Options::default()
        },
        None,
    );
    let files = files.unwrap_or_else(|| {
        (
            Arc::new(crate::start::spool::Host),
            Arc::new(crate::sink::fs::Host),
        )
    });
    #[cfg(feature = "test-hooks")]
    {
        crate::execution::trace::arm_pools(
            runtimes.controller.handle(),
            runtimes.finisher.handle(),
            host.log.clone(),
        );
        if let Err(refused) = super::hooks::arm_abort(&hooks, host.log.as_ref().map(|log| log.fd()))
        {
            host.hand(running, Error::startup(refused.0, None));
        }
        super::hooks::arm_sockets(running.points.clone());
        running.points.at(&Point::Main);
    }
    let builder = std::thread::Builder::new()
        .name("batch-controller".into())
        .stack_size(STACK);
    let spawned = if failing == Some(Thread::Controller) {
        Err(Error::spawn())
    } else {
        builder
            .spawn(move || {
                register();
                let execution = held
                    .execution
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                match execution {
                    Some(execution) => threads::controller(
                        host, running, prepared, execution, &options, files, staging,
                    ),
                    None => host.request(StopReason::Internal),
                }
            })
            .map_err(|_| Error::spawn())
    };
    let handle = match spawned {
        Ok(handle) => handle,
        Err(error) => host.hand(running, error),
    };
    // Step 8: the join never returns; if it does, a stop `internal`.
    let _ = handle.join();
    host.request(StopReason::Internal);
    loop {
        std::thread::park();
    }
}
