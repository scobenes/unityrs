//! The finisher (`docs/plans/batch.md` 2.11): every manifest write after
//! `running`, on a normal end and on a stop alike, from the controller's
//! hand-over. It takes the stop once; 7.4's table, by the phase reached,
//! decides what it writes, and 9.1's precedence the code.
pub mod budget;
mod encode;
pub mod handover;
mod reduce;
mod stopped;
use crate::{
    job::RunFailure,
    lifetime::{SpoolDetail, Stop},
    manifest,
    phase::{Account, Error},
    phase1::PreparedRun,
    progress::Phase,
    reason::Code,
    sink::directory::Directory,
};
pub use handover::{FinishCause, HandOver, Selected, Spool, Terminal};
pub use stopped::{Finished, Finisher, Reconcile, run};

pub struct Identity<'a> {
    pub started_at: &'a str,
    pub memory_bound: u64,
    pub spool_peak: u64,
    pub vm_hwm: u64,
    pub warnings: Warnings,
}
/// The head's process warnings (7.3's `warnings`), read when the head is
/// built: a library run has none.
#[derive(Clone, Copy, Debug, Default)]
pub struct Warnings {
    /// Stderr lines the bounded writer dropped (`stderr_dropped`).
    pub stderr_dropped: u64,
    /// Whether the stdin watch ended without an EOF (`stdin_watch_ended`).
    pub stdin_watch_ended: bool,
    /// The process host's live counters, sampled as each head is encoded,
    /// the `running` head and the final one, so a drop or the stdin
    /// watch's end before either is in it; none, the two fields above as
    /// they are.
    pub sample: Option<fn() -> Warnings>,
}
impl Warnings {
    /// The warnings as a head encoded now records them.
    #[must_use]
    pub fn read(&self) -> Self {
        self.sample.map_or(*self, |sample| sample())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultEnd {
    pub code: Code,
    /// Last acknowledged head, even when the final rename may have happened.
    pub status: Option<&'static str>,
    /// The reason that set the code, as the `exit` line names it.
    pub reason: Option<&'static str>,
    /// A stop's detail (`spool_full`'s), when its reason set the code.
    pub detail: Option<&'static str>,
}
/// What a head's identity reads: the spec and its digest, from the
/// controller's working state or the frozen facts alike.
pub trait Header {
    fn spec(&self) -> &crate::spec::model::Spec;
    fn spec_sha256(&self) -> &[u8; 32];
}
impl Header for PreparedRun {
    fn spec(&self) -> &crate::spec::model::Spec {
        &self.spec
    }
    fn spec_sha256(&self) -> &[u8; 32] {
        &self.spec_sha256
    }
}
impl Header for crate::phase1::Facts {
    fn spec(&self) -> &crate::spec::model::Spec {
        &self.spec
    }
    fn spec_sha256(&self) -> &[u8; 32] {
        &self.spec_sha256
    }
}
pub fn running(
    prepared: &dyn Header,
    directory: &Directory,
    account: &Account,
    identity: &Identity<'_>,
) -> Result<(), Error> {
    account.publication(|| {
        let mut json = encode::head(
            prepared.spec(),
            prepared.spec_sha256(),
            account,
            identity,
            None,
            None,
            None,
        )?;
        directory.write_head(json.end()?, true).map_err(sink_error)
    })
}
pub(crate) fn running_bytes(
    prepared: &dyn Header,
    account: &Account,
    identity: &Identity<'_>,
) -> Result<crate::winners::json::Json, Error> {
    encode::head(
        prepared.spec(),
        prepared.spec_sha256(),
        account,
        identity,
        None,
        None,
        None,
    )
}
/// A hand-over's code against the stop the finisher took (9.1): a 7 yields
/// to a more severe stop, a 74 only to 70, a 69 to none. Equal to taking
/// the more severe by the precedence, since a stop's code is 6, 5, 74 or 70.
#[must_use]
pub fn reduce(code: Code, taken: Option<Stop>) -> Code {
    taken.map_or(code, |stop| code.worst(stop.reason.code()))
}
/// The normal finish over a context the caller froze, with no try budget.
pub fn normal(
    cx: &manifest::rows::Context<'_>,
    directory: &Directory,
    identity: &Identity<'_>,
    ended_at: &str,
) -> Result<ResultEnd, Error> {
    let mut rows = manifest::chunks::Directory {
        directory,
        run_id: &cx.facts.spec.run_id,
        attempt: cx.facts.spec.attempt,
    };
    Ok(normal_into(
        cx,
        &mut budget::Budgeted {
            rows: &mut rows,
            budget: budget::Budget::UNARMED,
            clock: &budget::BootClock,
            cut: false,
        },
        directory,
        identity,
        Some(ended_at),
        None,
    ))
}
pub trait Head {
    fn write(
        &self,
        bytes: &[u8],
        account: &Account,
        budget: budget::Budget<'_>,
    ) -> Result<(), crate::sink::Error>;
    fn sync(&self) -> Result<(), crate::sink::Error>;
    fn figures(&self) -> Option<(crate::sink::store::Figures, u64)> {
        None
    }
}
impl Head for Directory {
    fn write(
        &self,
        bytes: &[u8],
        _: &Account,
        _: budget::Budget<'_>,
    ) -> Result<(), crate::sink::Error> {
        self.write_head(bytes, false)
    }
    fn sync(&self) -> Result<(), crate::sink::Error> {
        self.syncfs()
    }
}
impl Head for crate::sink::Sink {
    fn write(
        &self,
        bytes: &[u8],
        account: &Account,
        budget: budget::Budget<'_>,
    ) -> Result<(), crate::sink::Error> {
        match self {
            Self::Directory(d) => d.write_head(bytes, false),
            Self::Store(s) => s.final_head(bytes, account, budget),
        }
    }
    fn sync(&self) -> Result<(), crate::sink::Error> {
        match self {
            Self::Directory(d) => d.syncfs(),
            Self::Store(_) => Ok(()),
        }
    }
    fn figures(&self) -> Option<(crate::sink::store::Figures, u64)> {
        match self {
            Self::Store(s) => Some((s.figures(), s.unsettled())),
            Self::Directory(_) => None,
        }
    }
}
fn normal_into(
    cx: &manifest::rows::Context<'_>,
    destination: &mut budget::Budgeted<'_>,
    directory: &dyn Head,
    identity: &Identity<'_>,
    ended_at: Option<&str>,
    taken: Option<Stop>,
) -> ResultEnd {
    // Canonical stop was checked before the hand-over. Later stops cannot
    // reinterpret this snapshot. The finisher's own budget is a separate query.
    let reduction = cx.account.publication(|| reduce::normal(cx));
    let failure = reduction.err().map(|error| match error {
        Error::Run(f) => f,
        Error::Stopped => RunFailure::new(crate::attempt::ReasonCode::Internal),
    });
    conclude(
        cx,
        destination,
        directory,
        identity,
        ended_at,
        Ending {
            code: reduction.unwrap_or(Code::Complete),
            failure,
            stop: None,
            taken,
            phase: Phase::Publish,
            missing: None,
        },
    )
}
/// What the final head starts from: a normal reduction, or the stop taken.
#[derive(Clone, Copy)]
struct Ending<'a> {
    missing: Option<&'a str>,
    code: Code,
    failure: Option<RunFailure>,
    stop: Option<Stop>,
    /// The immutable hand-over capture; normal rows ignore it, but a
    /// superseded final head still meets its code by 9.1's precedence.
    taken: Option<Stop>,
    phase: Phase,
}
/// Step 3: every row file the context allows, `syncfs`, then the head.
fn conclude(
    cx: &manifest::rows::Context<'_>,
    destination: &mut budget::Budgeted<'_>,
    directory: &dyn Head,
    identity: &Identity<'_>,
    ended_at: Option<&str>,
    ending: Ending<'_>,
) -> ResultEnd {
    let mut code = ending.code;
    let mut report = manifest::finishing(cx, destination);
    // A stop's finish that could not read or build its rows writes no head
    // over `running` (7.4): a reader keeps the head that is true.
    if let (Some(stop), Some(failure)) = (ending.stop, report.failure) {
        return stopped::unwritten(stop, failure);
    }
    if let Some(failure) = ending.failure {
        RunFailure::reduce(&mut report.failure, failure);
    }
    if ended_at.is_none() {
        RunFailure::reduce(
            &mut report.failure,
            RunFailure::new(crate::attempt::ReasonCode::HostIo),
        );
    }
    if report.publication_failed {
        code = code.worst(Code::Incomplete);
    }
    if directory.sync().is_err() {
        report.sync_failed();
    }
    if let Some(failure) = report.failure {
        code = code.worst(failure.code());
    }
    // The head's stop: the stop taken, or `host_io` when a final `syncfs`
    // failed and is the more severe (7.4); a normal end's only on that sync.
    let synced = report.failure.filter(|_| report.filesystem_sync_failed);
    let shown = match (ending.stop, synced) {
        (Some(stop), Some(f)) if f.code().severity() < stop.reason.code().severity() => {
            Some((f.name(), None))
        }
        (Some(stop), _) => Some((stop.reason.name(), stop.detail.map(SpoolDetail::name))),
        (None, f) => f
            .map(|f| (RunFailure::name(f), None))
            .or_else(|| ending.missing.map(|at| ("unknown_prefab_key", Some(at)))),
    };
    // The exit line names the reason that set the code.
    let (reason, detail) = match ending.stop {
        Some(stop) if stop.reason.code() == code => {
            (Some(stop.reason.name()), stop.detail.map(SpoolDetail::name))
        }
        _ => (
            report.failure.map(RunFailure::name).or_else(|| {
                ending
                    .missing
                    .filter(|_| code == Code::Incomplete)
                    .map(|_| "unknown_prefab_key")
            }),
            None,
        ),
    };
    let status = code.status();
    let mut superseded = false;
    let published = cx.account.publication(|| {
        let mut json = encode::head(
            &cx.facts.spec,
            &cx.facts.spec_sha256,
            cx.account,
            identity,
            Some(&encode::Final {
                report: &report,
                code,
                phase: ending.phase,
                stop: shown,
            }),
            ended_at,
            directory.figures(),
        )?;
        directory
            .write(json.end()?, cx.account, destination.budget)
            .map_err(|e| {
                superseded = e.class == crate::sink::Class::Superseded;
                sink_error(e)
            })
    });
    let end = ResultEnd {
        code,
        status,
        reason,
        detail,
    };
    match published {
        Ok(()) => end,
        Err(error) => failed_head(end, error, superseded, ending.taken),
    }
}
fn failed_head(end: ResultEnd, error: Error, superseded: bool, taken: Option<Stop>) -> ResultEnd {
    let failure_code = match error {
        Error::Run(f) => f.code(),
        Error::Stopped if superseded => Code::Superseded,
        Error::Stopped => Code::Incomplete,
    };
    // Ordinary final-head filesystem failures contribute 4; allocation
    // and internal failures keep their stronger classification.
    let final_code = end.code.worst(failure_code);
    let final_code = if superseded {
        reduce(final_code, taken)
    } else {
        final_code
    };
    let by_stop = taken.filter(|stop| superseded && stop.reason.code() == final_code);
    let (reason, detail) = if let Some(stop) = by_stop {
        (Some(stop.reason.name()), stop.detail.map(SpoolDetail::name))
    } else if final_code == end.code {
        (end.reason, end.detail)
    } else if final_code == Code::Superseded {
        (Some("superseded"), None)
    } else if final_code == Code::Incomplete {
        (Some("publish_failed"), None)
    } else {
        match error {
            Error::Run(f) => (Some(f.name()), None),
            Error::Stopped => (end.reason, end.detail),
        }
    };
    ResultEnd {
        code: final_code,
        status: if superseded { None } else { Some("running") },
        reason,
        detail,
    }
}
fn sink_error(e: crate::sink::Error) -> Error {
    match e.class {
        crate::sink::Class::PublishFailed
        | crate::sink::Class::Superseded
        | crate::sink::Class::SinkUnreachable => Error::Stopped,
        crate::sink::Class::HostMemory => Error::memory(),
        crate::sink::Class::InvalidPath => Error::internal(),
        _ => Error::Run(RunFailure::new(crate::attempt::ReasonCode::HostIo)),
    }
}
