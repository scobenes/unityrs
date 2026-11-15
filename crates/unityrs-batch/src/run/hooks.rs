//! The official test hooks (`docs/plans/batch.md` 2.11, *Test hooks*),
//! compiled only under `test-hooks`: one [`TestHooks`] value, which the
//! binary reads from its variables at step 1 ([`TestHooks::from_variables`]) and
//! a library-API child sets itself. Unset, a hook does nothing.
//!
//! The environment sets `trace`, `handover`, `point`, `spawn`,
//! `start_count`, `init`, `files` and `host_root` ([`super::faults`]), and
//! `cutoff_in` (`UNITYRS_TEST_CUTOFF_IN`, milliseconds): step 6 reads the
//! wall clock as the deadline less the grace less that, so the deadline's
//! cutoff falls that long after step 6 however long startup took.
//! `skip_host_checks` and `tally` are the library API's alone; `resolve`
//! and `extra_roots` are read by URL activation, `zip_offset` by archive
//! activation.
//!
//! The `trace` log's lines: `dispatch <input id>`, `peek <bundle id>`,
//! `rename <input id>`, the controller's `handover`, the finisher's
//! `commit found at take: <id>` and `commit unknown at take: <id>
//! <errno>`, and `<point> reached` before each `wait` hold. The
//! `handover` report: one `stopped <input id>` line per input the finisher
//! writes `stopped`, then `culprit input <id>` or `culprit bundle <id>`.
use super::process::Thread;
use crate::{
    control::Culprit, controller, lifetime::Stop, phase1::Facts, publish, spec::model::Spec,
};
use std::{
    ffi::OsString,
    fs::File,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
};

/// Every row of 2.11's table.
#[derive(Default)]
pub struct TestHooks {
    /// `UNITYRS_TEST_TRACE`: the controller's and finisher's log; the
    /// lifecycle's two plain trace points go to `<trace>.lifecycle`.
    pub trace: Option<PathBuf>,
    /// `UNITYRS_TEST_HANDOVER`: the finisher's report of its snapshot.
    pub handover: Option<PathBuf>,
    /// `UNITYRS_TEST_POINT`: armable points only, by type.
    pub point: Vec<(Point, Action)>,
    /// `UNITYRS_TEST_SPAWN`: the thread whose spawn fails.
    pub spawn: Option<Thread>,
    /// `UNITYRS_TEST_START_COUNT`: start tasks that add nothing.
    pub start_count: usize,
    /// `UNITYRS_TEST_INIT`: step 4 records the engine as init.
    pub init: bool,
    /// The spool's, the sink's and the started spool's filesystems.
    pub files: Option<FileSystems>,
    /// Step 6's host facts in place of the system's.
    pub host_root: Option<Box<dyn crate::start::host::Host + Send + Sync>>,
    pub skip_host_checks: bool,
    /// A test's tally: every engine thread registers with it (pool threads
    /// through `on_thread_start`), the lifecycle thread and the stdin
    /// watch marking it before their first `poll`.
    pub tally: Option<Tally>,
    /// The deadline's cutoff this long after step 6 (milliseconds).
    pub cutoff_in: Option<u64>,
    /// `UNITYRS_TEST_RESOLVE` and `UNITYRS_TEST_EXTRA_ROOTS`, as read.
    pub resolve: Option<OsString>,
    pub extra_roots: Option<PathBuf>,
    pub zip_offset: Option<u64>,
}
/// The `tally` hook's two calls.
#[derive(Clone, Copy)]
pub struct Tally {
    pub register: fn(),
    pub mark: fn(),
}
/// The `files` hook's adapters over the engine's own filesystem traits.
pub struct FileSystems {
    pub spool: Arc<dyn crate::start::spool::FileSystem + Send + Sync>,
    pub sink: Arc<dyn crate::sink::fs::FileSystem>,
    pub started: Arc<dyn crate::spool::FileSystem>,
}
/// An armable point. The lifecycle's two trace points are not among them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Point {
    /// A bundle's local copy, an input's attempt, a bundle's peek.
    Stage(String),
    Attempt(String),
    Peek(String),
    /// An input's commit: after the slot is set, before the stop-word
    /// load; and between its rename's `trace` line and its bit's clear.
    Commit(String),
    Renamed(String),
    /// A worker after it queued a named input's or bundle's end, its panic
    /// line queued before (2.11's *Panics*), before its wake.
    End(String),
    /// The controller's wait, before its poll.
    Poll,
    /// Phase 7's transfer, after the transfers drain, after verification.
    Transfer,
    Drained,
    Verified,
    /// The finisher, before it takes the reason.
    Finisher,
    /// The controller's drain of the queued stderr lines, before each
    /// write, its drain lock held (2.11's *Panics*): a `wait` there is a
    /// write held as writeback holds a regular file's.
    Drain,
    /// The main thread before its controller spawn, the progress thread
    /// at its start, the lifecycle thread and the stdin watch at their
    /// marks.
    Main,
    Progress,
    Lifecycle,
    Stdin,
    /// `exit_now`, after its resets: `spin` and `wait` only.
    ExitNow,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Panic,
    Spin,
    Wait(PathBuf),
}
#[derive(Debug, PartialEq, Eq)]
pub struct Refused(pub &'static str);
impl TestHooks {
    /// Step 1's read. A malformed variable, or a `point` naming one of the
    /// lifecycle's two trace points, is refused (69 `startup_failed`).
    pub fn from_variables() -> Result<Self, Refused> {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let text = |name: &'static str| {
            var(name)
                .map(|v| v.into_string().map_err(|_| Refused(name)))
                .transpose()
        };
        let mut hooks = Self {
            trace: var("UNITYRS_TEST_TRACE").map(PathBuf::from),
            handover: var("UNITYRS_TEST_HANDOVER").map(PathBuf::from),
            init: var("UNITYRS_TEST_INIT").is_some(),
            resolve: var("UNITYRS_TEST_RESOLVE"),
            extra_roots: var("UNITYRS_TEST_EXTRA_ROOTS").map(PathBuf::from),
            ..Self::default()
        };
        if let Some(name) = text("UNITYRS_TEST_SPAWN")? {
            hooks.spawn = Some(match name.as_str() {
                "progress" => Thread::Progress,
                "finisher" => Thread::Finisher,
                "lifecycle" => Thread::Lifecycle,
                "worker" => Thread::Worker,
                "controller" => Thread::Controller,
                _ => return Err(Refused("UNITYRS_TEST_SPAWN")),
            });
        }
        if let Some(n) = text("UNITYRS_TEST_START_COUNT")? {
            hooks.start_count = n.parse().map_err(|_| Refused("UNITYRS_TEST_START_COUNT"))?;
        }
        if let Some(list) = text("UNITYRS_TEST_POINT")? {
            hooks.point = points(&list).ok_or(Refused("UNITYRS_TEST_POINT"))?;
        }
        if let Some(n) = text("UNITYRS_TEST_CUTOFF_IN")? {
            hooks.cutoff_in = Some(n.parse().map_err(|_| Refused("UNITYRS_TEST_CUTOFF_IN"))?);
        }
        let root = var("UNITYRS_TEST_HOST_ROOT").map(PathBuf::from);
        if let Some(root) = &root {
            hooks.host_root = Some(Box::new(super::faults::Rooted(root.clone())));
        }
        let plan = var("UNITYRS_TEST_FILES")
            .map(|path| {
                super::faults::Plan::read(path.as_ref()).ok_or(Refused("UNITYRS_TEST_FILES"))
            })
            .transpose()?;
        if plan.is_some() || root.is_some() {
            let files = Arc::new(super::faults::Files { plan, root });
            hooks.files = Some(FileSystems {
                spool: files.clone(),
                sink: files.clone(),
                started: files,
            });
        }
        Ok(hooks)
    }
    /// The `trace` log, opened for appends at step 1.
    pub(super) fn trace_log(&self) -> std::io::Result<Option<Arc<Log>>> {
        self.trace
            .as_ref()
            .map(|path| Log::append(path).map(Arc::new))
            .transpose()
    }
}
/// `<point>=<action>,...`. `exit_now` takes `spin` and `wait` alone.
fn points(list: &str) -> Option<Vec<(Point, Action)>> {
    list.split(',')
        .map(|entry| {
            let (point, action) = entry.split_once('=')?;
            let action = match action {
                "panic" => Action::Panic,
                "spin" => Action::Spin,
                _ => Action::Wait(PathBuf::from(action.strip_prefix("wait:")?)),
            };
            let named = |prefix: &str| point.strip_prefix(prefix).map(str::to_owned);
            let point = match point {
                "poll" => Point::Poll,
                "transfer" => Point::Transfer,
                "drained" => Point::Drained,
                "verified" => Point::Verified,
                "finisher" => Point::Finisher,
                "drain" => Point::Drain,
                "main" => Point::Main,
                "progress" => Point::Progress,
                "lifecycle" => Point::Lifecycle,
                "stdin" => Point::Stdin,
                "exit_now" if action != Action::Panic => Point::ExitNow,
                _ => {
                    if let Some(id) = named("stage:") {
                        Point::Stage(id)
                    } else if let Some(id) = named("attempt:") {
                        Point::Attempt(id)
                    } else if let Some(id) = named("peek:") {
                        Point::Peek(id)
                    } else if let Some(id) = named("commit:") {
                        Point::Commit(id)
                    } else if let Some(id) = named("end:") {
                        Point::End(id)
                    } else {
                        Point::Renamed(named("renamed:")?)
                    }
                }
            };
            Some((point, action))
        })
        .collect()
}
/// The `trace` log: each line one `write` of its whole text, appended.
pub struct Log(File);
impl Log {
    /// `path` opened for appends, created when missing.
    pub fn append(path: &std::path::Path) -> std::io::Result<Self> {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(path)
            .map(Self)
    }
    /// Its descriptor, for `exit_now`'s hold, which writes it from a handler.
    #[must_use]
    pub fn fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.0.as_raw_fd()
    }
    pub fn line(&self, args: std::fmt::Arguments<'_>) {
        let mut line = [0_u8; 512];
        let mut rest = &mut line[..];
        if writeln!(rest, "{args}").is_ok() {
            let used = 512 - rest.len();
            let _ = (&self.0).write_all(line.get(..used).unwrap_or_default());
        }
    }
}
/// The points armed for one run, resolved against its spec, and the
/// `trace` log: the controller's, publication's and the body's hooks.
pub struct Points {
    spec: Arc<Spec>,
    armed: Vec<(Point, Action, String, AtomicBool)>,
    log: Option<Arc<Log>>,
}
impl Points {
    pub(super) fn new(hooks: &TestHooks, spec: Arc<Spec>, log: Option<Arc<Log>>) -> Self {
        let armed = hooks
            .point
            .iter()
            .map(|(p, a)| (p.clone(), a.clone(), name(p), AtomicBool::new(false)))
            .collect();
        Self { spec, armed, log }
    }
    fn id(&self, bundle: u32) -> &str {
        usize::try_from(bundle)
            .ok()
            .and_then(|b| self.spec.bundle(b))
            .map_or("", |b| b.id())
    }
    /// Hold at `point` when it is armed, once per entry.
    ///
    /// # Panics
    ///
    /// When its action is `panic`: the hook's purpose.
    pub fn at(&self, point: &Point) {
        for (armed, action, name, fired) in &self.armed {
            if armed != point || fired.swap(true, SeqCst) {
                continue;
            }
            match action {
                #[allow(clippy::panic)] // The `panic` action's whole purpose.
                Action::Panic => panic!("test hook: {name}"),
                Action::Spin => loop {
                    std::hint::spin_loop();
                },
                Action::Wait(path) => {
                    if let Some(log) = &self.log {
                        log.line(format_args!("{name} reached"));
                    }
                    while !path.exists() {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
        }
    }
    fn named(&self, make: fn(String) -> Point, bundle: u32) {
        if !self.armed.is_empty() {
            self.at(&make(self.id(bundle).to_owned()));
        }
    }
    pub fn trace(&self, args: std::fmt::Arguments<'_>) {
        if let Some(log) = &self.log {
            log.line(args);
        }
    }
}
fn name(point: &Point) -> String {
    match point {
        Point::Stage(id) => format!("stage:{id}"),
        Point::Attempt(id) => format!("attempt:{id}"),
        Point::Peek(id) => format!("peek:{id}"),
        Point::Commit(id) => format!("commit:{id}"),
        Point::Renamed(id) => format!("renamed:{id}"),
        Point::End(id) => format!("end:{id}"),
        Point::Poll => "poll".into(),
        Point::Transfer => "transfer".into(),
        Point::Drained => "drained".into(),
        Point::Verified => "verified".into(),
        Point::Finisher => "finisher".into(),
        Point::Drain => "drain".into(),
        Point::Main => "main".into(),
        Point::Progress => "progress".into(),
        Point::Lifecycle => "lifecycle".into(),
        Point::Stdin => "stdin".into(),
        Point::ExitNow => "exit_now".into(),
    }
}
impl unityrs_export::ngui::Hooks for Points {}
impl controller::Hooks for Points {
    fn event(&self, event: controller::Event) {
        match event {
            controller::Event::AttemptDispatched(i) => {
                self.trace(format_args!("dispatch {}", self.id(i)));
            }
            controller::Event::PeekDispatched(b) => {
                self.trace(format_args!("peek {}", self.id(b)));
            }
            controller::Event::Renamed(i) => self.trace(format_args!("rename {}", self.id(i))),
            _ => {}
        }
    }
    fn point(&self, point: controller::Point) {
        match point {
            controller::Point::Stage(b, _) => self.named(Point::Stage, b),
            controller::Point::Attempt(i, _) => self.named(Point::Attempt, i),
            controller::Point::Peek(b, _) => self.named(Point::Peek, b),
            controller::Point::BeforeCommitLoad(i) => self.named(Point::Commit, i),
            controller::Point::AfterRenameTrace(i) => self.named(Point::Renamed, i),
            controller::Point::EndQueued(_, i) => self.named(Point::End, i),
            controller::Point::BeforePoll => self.at(&Point::Poll),
            _ => {}
        }
    }
}
impl publish::Hooks for Points {
    fn point(&self, point: publish::Point) {
        match point {
            publish::Point::Transfer => self.at(&Point::Transfer),
            publish::Point::Drained => self.at(&Point::Drained),
            _ => {}
        }
    }
}
impl super::Observer for Points {
    fn seam(&self, seam: super::Seam) {
        if seam == super::Seam::Verified {
            self.at(&Point::Verified);
        }
    }
}
/// The `handover` report, from the finisher's snapshot: every input whose
/// uncommitted bit is set but a commit its one stat found, and the culprit.
pub(super) fn report(
    path: &std::path::Path,
    facts: &Facts,
    found: Option<u32>,
    stop: Option<Stop>,
) {
    use std::fmt::Write as _;
    let mut text = String::new();
    let id = |i: u32| {
        usize::try_from(i)
            .ok()
            .and_then(|i| facts.spec.bundle(i))
            .map_or("", |b| b.id())
    };
    for i in 0..u32::try_from(facts.uncommitted.len()).unwrap_or(0) {
        let set = usize::try_from(i)
            .ok()
            .and_then(|i| facts.uncommitted.get(i))
            .unwrap_or(false);
        if set && found != Some(i) {
            let _ = writeln!(text, "stopped {}", id(i));
        }
    }
    let _ = match stop.map(|s| s.culprit) {
        Some(Culprit::Input(i)) => writeln!(text, "culprit input {}", id(i)),
        Some(Culprit::Bundle(b)) => writeln!(text, "culprit bundle {}", id(b)),
        _ => Ok(()),
    };
    let _ = std::fs::write(path, text);
}
/// The lifecycle thread's mark ([`crate::lifetime::Watch::mark`] takes a
/// plain `fn`): the tally's registration and mark, then its point.
static LIFECYCLE: std::sync::OnceLock<(Option<Tally>, Arc<Points>)> = std::sync::OnceLock::new();
pub(super) fn arm_lifecycle(tally: Option<Tally>, points: Arc<Points>) {
    let _ = LIFECYCLE.set((tally, points));
}
pub(super) fn lifecycle_mark() {
    if let Some((tally, points)) = LIFECYCLE.get() {
        if let Some(tally) = tally {
            (tally.register)();
            (tally.mark)();
        }
        points.at(&Point::Lifecycle);
    }
}
