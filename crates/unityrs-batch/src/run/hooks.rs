//! The official test hooks (`docs/plans/batch.md` 2.11, *Test hooks*),
//! compiled only under `test-hooks`: one [`TestHooks`] value, which the
//! binary reads from its variables at step 1 ([`TestHooks::from_variables`]) and
//! a library-API child sets itself. Unset, a hook does nothing.
//!
//! The environment sets `trace`, `handover`, `point`, `spawn`,
//! `start_count`, `init`, `files` and `host_root` ([`super::faults`]), and
//! `cutoff_in` (`UNITYRS_TEST_CUTOFF_IN`, milliseconds): step 6 reads the
//! wall clock as the deadline less the grace less that, so the deadline's
//! cutoff falls that long after step 6 however long startup took. With
//! `UNITYRS_TEST_CUTOFF_ARM`, a FIFO byte starts the delay instead.
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
#[path = "hooks_memory.rs"]
pub mod memory;
#[path = "hooks_network.rs"]
mod network;
use super::process::Thread;
use crate::{
    control::Culprit, controller, lifetime::Stop, phase1::Facts, publish, spec::model::Spec,
};
pub use network::{Resolve, extra_roots};
use std::{
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
    /// `UNITYRS_TEST_CUTOFF_ARM`: with `cutoff_in`, arm that delay only
    /// after a byte arrives on this FIFO, acknowledging an origin request.
    pub cutoff_arm: Option<PathBuf>,
    /// `UNITYRS_TEST_RESOLVE`: parsed answers; absent means system DNS.
    pub resolve: Option<Resolve>,
    /// `UNITYRS_TEST_EXTRA_ROOTS`: certificates loaded at startup.
    pub extra_roots: Vec<rustls::pki_types::CertificateDer<'static>>,
    pub zip_offset: Option<u64>,
    /// Library-only phase-6 run cuts; validation retains production maxima.
    pub sort_options: Option<crate::sort::Options>,
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
    /// SIGABRT's handler, after resets and before its final abort: only
    /// `spin` or `wait`, with an async-signal-safe hold.
    AbortHandler,
    /// A socket's late publication and its checks around connect, for
    /// deterministic races with the lifecycle's closing flag.
    SocketPublish,
    SocketConnect,
    SocketConnected,
    /// The same socket seams, restricted to one request permit's table
    /// entry so an earlier active connection does not take a late hold.
    SocketPublishPermit(usize),
    SocketConnectPermit(usize),
    SocketConnectedPermit(usize),
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
            cutoff_arm: var("UNITYRS_TEST_CUTOFF_ARM").map(PathBuf::from),
            init: var("UNITYRS_TEST_INIT").is_some(),
            resolve: text("UNITYRS_TEST_RESOLVE")?
                .map(|value| Resolve::parse(&value))
                .transpose()?,
            extra_roots: var("UNITYRS_TEST_EXTRA_ROOTS")
                .map(|path| extra_roots(path.as_ref()))
                .transpose()?
                .unwrap_or_default(),
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
                "abort_handler" if action != Action::Panic => Point::AbortHandler,
                "socket_publish" => Point::SocketPublish,
                "socket_connect" => Point::SocketConnect,
                "socket_connected" => Point::SocketConnected,
                _ if point.starts_with("socket_") => socket_point(point)?,
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
fn socket_point(point: &str) -> Option<Point> {
    let (label, index) = point.split_once(':')?;
    if index.is_empty() || !index.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let index = index.parse::<usize>().ok()?;
    if index >= crate::sys::reset::SOCKET_CAPACITY {
        return None;
    }
    Some(match label {
        "socket_publish" => Point::SocketPublishPermit(index),
        "socket_connect" => Point::SocketConnectPermit(index),
        "socket_connected" => Point::SocketConnectedPermit(index),
        _ => return None,
    })
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
    sort_options: Option<crate::sort::Options>,
    spec: Arc<Spec>,
    armed: Vec<(Point, Action, String, AtomicBool)>,
    log: Option<Arc<Log>>,
}
/// Install the socket callbacks before the controller starts. The process
/// retains its armed points through exit; callbacks run outside reset.
pub(super) fn arm_sockets(points: Arc<Points>) {
    static POINTS: std::sync::OnceLock<Arc<Points>> = std::sync::OnceLock::new();
    fn at(point: crate::sys::reset::SocketPoint, index: usize) {
        use crate::sys::reset::SocketPoint;
        if let Some(points) = POINTS.get() {
            let (point, target, label) = match point {
                SocketPoint::BeforePublish => (
                    Point::SocketPublish,
                    Point::SocketPublishPermit(index),
                    "socket_publish",
                ),
                SocketPoint::BeforeConnect => (
                    Point::SocketConnect,
                    Point::SocketConnectPermit(index),
                    "socket_connect",
                ),
                SocketPoint::AfterConnect => (
                    Point::SocketConnected,
                    Point::SocketConnectedPermit(index),
                    "socket_connected",
                ),
                SocketPoint::ClosingRejected => {
                    points.trace(format_args!("socket rejected"));
                    return;
                }
            };
            if points.fire(&point) {
                points.trace(format_args!("{label} released"));
            }
            if points.fire(&target) {
                points.trace(format_args!("{label}:{index} released"));
            }
        }
    }
    let _ = POINTS.set(points);
    crate::sys::reset::arm_socket_points(at);
}
/// Arm the async-signal-safe SIGABRT hold at step 7. Its path storage is
/// allocated fallibly before a controller can open any socket.
pub(super) fn arm_abort(hooks: &TestHooks, trace: Option<i32>) -> Result<(), Refused> {
    let Some((_, action)) = hooks.point.iter().find(|(p, _)| *p == Point::AbortHandler) else {
        return Ok(());
    };
    let wait = match action {
        Action::Wait(path) => {
            let bytes = path.as_os_str().as_encoded_bytes();
            let length = bytes
                .len()
                .checked_add(1)
                .ok_or(Refused("UNITYRS_TEST_POINT"))?;
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(length)
                .map_err(|_| Refused("UNITYRS_TEST_POINT"))?;
            owned.extend_from_slice(bytes);
            owned.push(0);
            Some(
                std::ffi::CString::from_vec_with_nul(owned)
                    .map_err(|_| Refused("UNITYRS_TEST_POINT"))?,
            )
        }
        Action::Spin => None,
        Action::Panic => return Err(Refused("UNITYRS_TEST_POINT")),
    };
    crate::sys::signals::arm_abort(wait, trace);
    Ok(())
}
impl Points {
    pub(super) fn new(hooks: &TestHooks, spec: Arc<Spec>, log: Option<Arc<Log>>) -> Self {
        let armed = hooks
            .point
            .iter()
            .map(|(p, a)| (p.clone(), a.clone(), name(p), AtomicBool::new(false)))
            .collect();
        Self {
            spec,
            armed,
            log,
            sort_options: hooks.sort_options,
        }
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
        self.fire(point);
    }
    fn fire(&self, point: &Point) -> bool {
        let mut reached = false;
        for (armed, action, name, fired) in &self.armed {
            if armed != point || fired.swap(true, SeqCst) {
                continue;
            }
            reached = true;
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
        reached
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
        Point::SocketPublish => "socket_publish".into(),
        Point::SocketConnect => "socket_connect".into(),
        Point::SocketConnected => "socket_connected".into(),
        Point::SocketPublishPermit(index) => format!("socket_publish:{index}"),
        Point::SocketConnectPermit(index) => format!("socket_connect:{index}"),
        Point::SocketConnectedPermit(index) => format!("socket_connected:{index}"),
        Point::AbortHandler => "abort_handler".into(),
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
    fn sort_options(&self) -> crate::sort::Options {
        self.sort_options.unwrap_or_default()
    }
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

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn handler_points_refuse_panic_and_accept_only_wait_or_spin() {
        assert!(points("abort_handler=panic").is_none());
        assert!(points("exit_now=panic").is_none());
        assert_eq!(
            points("abort_handler=spin,abort_handler=wait:release"),
            Some(vec![
                (Point::AbortHandler, Action::Spin),
                (Point::AbortHandler, Action::Wait(PathBuf::from("release"))),
            ]),
        );
        for (label, point) in [
            ("socket_publish", Point::SocketPublish),
            ("socket_connect", Point::SocketConnect),
            ("socket_connected", Point::SocketConnected),
        ] {
            assert_eq!(
                points(&format!("{label}=spin")),
                Some(vec![(point, Action::Spin)])
            );
        }
    }

    #[test]
    fn socket_points_accept_only_in_range_decimal_permit_indexes() {
        for (label, point) in [
            ("socket_publish", Point::SocketPublishPermit(0)),
            ("socket_connect", Point::SocketConnectPermit(0)),
            ("socket_connected", Point::SocketConnectedPermit(0)),
        ] {
            assert_eq!(
                points(&format!("{label}:0=spin")),
                Some(vec![(point, Action::Spin)])
            );
            for index in ["", "-1", "+1", "a", "1:2", "514", "18446744073709551616"] {
                assert!(points(&format!("{label}:{index}=spin")).is_none());
            }
        }
        assert_eq!(
            points("socket_publish:513=wait:release"),
            Some(vec![(
                Point::SocketPublishPermit(513),
                Action::Wait(PathBuf::from("release"))
            )]),
        );
    }
}
