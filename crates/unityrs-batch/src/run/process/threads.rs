//! The controller's and the finisher's threads (2.4's hand-over, 2.11's
//! *The finisher*). After the `running` head nothing here allocates: the
//! ids are borrowed from the hand-over's facts, which live until `_exit`,
//! and every line is formatted into fixed storage.
#[cfg(feature = "test-hooks")]
use super::super::hooks::Point;
use super::{
    super::{Body, Ending, Error, Options, Parked, errors, finish_handover, stamp, terminal},
    Host, Running,
};
use crate::{
    diagnostic::{self, LINE_BYTES, Secrets, Writer},
    execution::Execution,
    finish::{FinishCause, HandOver, Reconcile, ResultEnd},
    lifetime::{Boot, Stop},
    phase1::PreparedRun,
    progress::Exit,
    reason::{Code, StopReason},
    sys::RegisteredWake,
};
use std::{
    io::Write,
    sync::{Arc, PoisonError},
    time::{Duration, Instant},
};

/// The filesystems the controller's body uses: the hosts', or the `files`
/// hook's.
pub(super) type Files = (
    Arc<dyn crate::start::spool::FileSystem + Send + Sync>,
    Arc<dyn crate::sink::fs::FileSystem>,
);
/// An errno as details write it: its name, else its decimal value.
struct Errno(Option<i32>);
impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(n) => match errors::errno_name(n) {
                Some(name) => f.write_str(name),
                None => write!(f, "{n}"),
            },
            None => f.write_str("unknown"),
        }
    }
}
/// One stderr line through `writer`, formatted into fixed storage, cut at
/// `PIPE_BUF`. `immediate` is an exit at once's route: nonblocking and
/// drop-only, never a regular file's write, a drop counted
/// ([`Writer::line`]'s `lifecycle` route).
pub(super) fn write_line(
    writer: &Writer,
    secrets: &Secrets<'_>,
    args: std::fmt::Arguments<'_>,
    immediate: bool,
) {
    let mut text = [0_u8; LINE_BYTES];
    let mut rest = &mut text[..];
    let _ = rest.write_fmt(args);
    let used = LINE_BYTES - rest.len();
    let bytes = text.get(..used).unwrap_or_default();
    let text = std::str::from_utf8(bytes).unwrap_or_else(|e| {
        std::str::from_utf8(bytes.get(..e.valid_up_to()).unwrap_or_default()).unwrap_or_default()
    });
    let mut buffer = [0; LINE_BYTES];
    writer.line(text, secrets, &mut buffer, immediate);
}
/// The queued panic lines (2.4), through `writer`, waiting for another
/// drain until `until` (`None`: as long as it writes; past it, the lines
/// left are counted, unwritten: [`diagnostic::pending::drain`]), `before`
/// called before each write (the controller's `drain` hook).
pub(super) fn write_pending(
    writer: &Writer,
    secrets: &Secrets<'_>,
    immediate: bool,
    until: Option<Instant>,
    mut before: impl FnMut(),
) {
    diagnostic::pending::drain(until, |line| {
        before();
        write_line(writer, secrets, format_args!("{line}"), immediate);
    });
}
impl Host {
    /// The successful startup's warning runs on main after controller spawn,
    /// so stderr can hold neither progress, handover nor the lifecycle.
    /// No detached early-writer stack overlaps the full engine topology.
    pub(super) fn note(&'static self, pending: Option<diagnostic::PendingLine<'_>>) {
        if let (Some(pending), Some(note)) = (pending, self.early.take_note()) {
            let text = std::str::from_utf8(note.bytes.get(..note.len).unwrap_or_default())
                .unwrap_or_default();
            let mut buffer = [0; LINE_BYTES];
            pending.within(text, &self.secrets(), &mut buffer, self.early.note_wait());
        }
    }
    pub(super) fn line(&'static self, args: std::fmt::Arguments<'_>) {
        write_line(self.writer, &self.secrets(), args, false);
    }
    /// How long a line of the finisher's may wait: at most 1 s, never
    /// past `T_hard` once armed (2.11's *The finisher*).
    pub(super) fn line_wait(&self) -> Duration {
        self.process
            .state
            .hard()
            .map_or(Duration::from_secs(1), |hard| {
                let now = Boot::now().map_or(u64::MAX, |b| b.0);
                Duration::from_millis(hard.0.saturating_sub(now)).min(Duration::from_secs(1))
            })
    }
    /// The run failures' lines their catch sites left (2.4), as the
    /// finisher or an exit at once writes them: after the hand-over, so no
    /// stop waits on stderr; by the immediate route on an exit at once;
    /// waiting for another drain (the controller's) no longer than a line
    /// of the finisher's may ([`Self::line_wait`]), so a write of its that
    /// nothing bounds (a regular file's) never holds the exit.
    pub(super) fn pending(&'static self, immediate: bool) {
        let until = Instant::now() + self.line_wait();
        write_pending(self.writer, &self.secrets(), immediate, Some(until), || {});
    }
}
/// The controller's thread: the body, its hand-over to the finisher, then
/// its park with the owners (2.4's hand-over, 2.11's *Panics*).
#[allow(clippy::too_many_arguments)] // The prebuilt process staging joins the existing owners.
pub(super) fn controller(
    host: &'static Host,
    running: &'static Running,
    prepared: PreparedRun,
    execution: Execution,
    options: &Options,
    files: Files,
    staging: Option<crate::controller::Staging>,
) -> ! {
    let sender = running.service.sender();
    let started_at = std::str::from_utf8(&running.started_at).unwrap_or_default();
    let (spool_fs, sink_fs) = files;
    let body = Body {
        bound: host.bound,
        progress: &sender,
        spool_fs: spool_fs.as_ref(),
        sink_fs,
        options,
        fin: &running.fin,
        started_at,
    };
    let mut parked = Parked::default();
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        body.body(
            prepared,
            &mut parked,
            Some(execution),
            staging,
            Some(host.store_services(running.policy.cutoff)?),
        )
    }));
    // Its own panic's line, queued before the finisher is released.
    if ran.is_err() {
        diagnostic::pending::panic(format_args!(""));
    }
    let outcome = match ran {
        Ok(Ok(())) => parked.handover.take().ok_or_else(Error::internal),
        Ok(Err(error)) => Err(error),
        Err(_) => {
            options
                .lifetime
                .request(Stop::external(StopReason::Internal));
            match parked.handover.take() {
                Some(mut handover) => {
                    // Publication's gate closed, as a normal hand-over's is.
                    if let Some(owned) = &parked.owned {
                        owned.gate.close();
                    }
                    handover.cause = FinishCause::Stop;
                    let phase = handover.phase;
                    stamp(&mut handover, phase);
                    Ok(handover)
                }
                None => Err(Error::internal()),
            }
        }
    };
    #[cfg(feature = "test-hooks")]
    host.trace(format_args!("handover"));
    *host.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    if host.fin.notify().is_err() {
        host.request(StopReason::Internal);
    }
    // The lines the run's catch sites left, its own among them, waiting
    // for another drain as long as it writes: the controller only parks
    // after.
    #[cfg(feature = "test-hooks")]
    let before = || running.points.at(&Point::Drain);
    #[cfg(not(feature = "test-hooks"))]
    let before = || {};
    write_pending(host.writer, &host.secrets(), false, None, before);
    // The owners stay parked until `_exit`.
    loop {
        std::thread::park();
    }
}
/// The finisher's wait for the hand-over on its registered wake.
fn handed(
    host: &Host,
    runtime: &tokio::runtime::Runtime,
    registered: &RegisteredWake,
) -> Result<HandOver, Error> {
    loop {
        if let Some(outcome) = host
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return outcome;
        }
        if runtime.block_on(registered.wait()).is_err() {
            host.request(StopReason::Internal);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
/// The finisher's thread (2.11's *The finisher*): its wait for the
/// hand-over, its take, the finish, the `exit` line and `exit_now`. Its
/// code so far is kept in [`Host::finishing`], outside its unwind.
pub(super) fn finisher(
    host: &'static Host,
    running: &'static Running,
    runtime: &tokio::runtime::Runtime,
    registered: &RegisteredWake,
) -> ! {
    let outcome = handed(host, runtime, registered);
    if let Err(error) = &outcome {
        host.finishing(error.code);
    }
    #[cfg(feature = "test-hooks")]
    running.points.at(&Point::Finisher);
    let state = &host.process.state;
    let taken = state.take();
    if taken.is_some()
        && let Ok(now) = Boot::now()
        && let Ok(hard) = running.policy.hard(now)
    {
        state.arm(hard);
    }
    // The hand-over lives in this frame until `_exit`: its owners outlive
    // the finisher, and the ids below borrow its facts.
    let mut kept = None;
    let mut unknown = None;
    let mut exit_inputs = None;
    let end = match outcome {
        Err(error) => Err(terminal(error, taken)),
        Ok(handover) => {
            let handover = kept.insert(handover);
            let sender = running.service.sender();
            let finished = finish_handover(
                handover,
                taken,
                &Ending {
                    state,
                    fin: &running.fin,
                    started_at: std::str::from_utf8(&running.started_at).unwrap_or_default(),
                    bound: host.bound,
                    progress: &sender,
                    warnings: super::SAMPLED,
                    wake: Some(registered),
                },
            );
            exit_inputs = Some(super::super::exit_inputs(
                host.snapshot(running),
                handover,
                &finished,
            ));
            let facts = handover.facts.as_deref();
            let id = |i: u32| {
                facts
                    .and_then(|f| usize::try_from(i).ok().and_then(|i| f.spec.bundle(i)))
                    .map_or("", |b| b.id())
            };
            match finished.reconcile {
                #[cfg(feature = "test-hooks")]
                Some(Reconcile::Found(i)) => {
                    host.trace(format_args!("commit found at take: {}", id(i)));
                }
                Some(Reconcile::Unknown(i, errno)) => {
                    #[cfg(feature = "test-hooks")]
                    host.trace(format_args!(
                        "commit unknown at take: {} {}",
                        id(i),
                        Errno(errno)
                    ));
                    unknown = Some((id(i), errno));
                }
                _ => {}
            }
            #[cfg(feature = "test-hooks")]
            if let (Some(path), Some(facts)) = (&host.handover, facts) {
                let found = match finished.reconcile {
                    Some(Reconcile::Found(i)) => Some(i),
                    _ => None,
                };
                super::super::hooks::report(path, facts, found, taken);
            }
            Ok(finished.end)
        }
    };
    let code = match &end {
        Ok(end) => end.code,
        Err(error) => error.code,
    };
    host.finishing(code);
    state.store_final(code);
    // At most 1 s, never past `T_hard` once armed.
    let wait = host.line_wait();
    let mut snapshot = host.snapshot(running);
    if let Some(inputs) = exit_inputs {
        snapshot.inputs = inputs;
    }
    let _ = running.service.exit(snapshot, exit_of(end), wait);
    if let Some((id, errno)) = unknown {
        host.line(format_args!(
            "unityrs batch: commit unknown at take: {id} {}\n",
            Errno(errno)
        ));
    }
    host.pending(false);
    #[cfg(feature = "test-hooks")]
    super::super::hooks::memory::at(super::super::hooks::memory::Point::Exit);
    host.process.exit(code)
}
/// The finisher's thread body around [`finisher`]: a panic exits with the
/// more severe of 70 and its code so far (2.11's *Panics*).
pub(super) fn finisher_thread(
    host: &'static Host,
    running: &'static Running,
    runtime: &'static tokio::runtime::Runtime,
    registered: &RegisteredWake,
) -> ! {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finisher(host, running, runtime, registered)
    }));
    // Its line, and any left queued, by the immediate route.
    diagnostic::pending::panic(format_args!("the finisher: "));
    host.pending(true);
    host.process.exit(Code::Internal.worst(host.code_so_far()))
}
/// A run's end as its `exit` line, allocating nothing: a stop's fixed
/// detail is borrowed, an error's moved.
#[must_use]
pub fn exit_of(end: Result<ResultEnd, Error>) -> Exit {
    match end {
        Ok(end) => Exit {
            status: end.status,
            code: end.code,
            reason: end.reason,
            detail: end.detail.map(std::borrow::Cow::Borrowed),
        },
        Err(error) => Exit {
            status: error.status,
            code: error.code,
            reason: Some(error.reason),
            detail: error.detail.map(std::borrow::Cow::Owned),
        },
    }
}
