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
        )?;
        directory.write_head(json.end()?, true).map_err(sink_error)
    })
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
    ))
}
fn normal_into(
    cx: &manifest::rows::Context<'_>,
    destination: &mut budget::Budgeted<'_>,
    directory: &Directory,
    identity: &Identity<'_>,
    ended_at: Option<&str>,
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
            phase: Phase::Publish,
        },
    )
}
/// What the final head starts from: a normal reduction, or the stop taken.
#[derive(Clone, Copy)]
struct Ending {
    code: Code,
    failure: Option<RunFailure>,
    stop: Option<Stop>,
    phase: Phase,
}
/// Step 3: every row file the context allows, `syncfs`, then the head.
fn conclude(
    cx: &manifest::rows::Context<'_>,
    destination: &mut budget::Budgeted<'_>,
    directory: &Directory,
    identity: &Identity<'_>,
    ended_at: Option<&str>,
    ending: Ending,
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
    if directory.syncfs().is_err() {
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
        (None, f) => f.map(|f| (RunFailure::name(f), None)),
    };
    // The exit line names the reason that set the code.
    let (reason, detail) = match ending.stop {
        Some(stop) if stop.reason.code() == code => {
            (Some(stop.reason.name()), stop.detail.map(SpoolDetail::name))
        }
        _ => (report.failure.map(RunFailure::name), None),
    };
    let status = code.status();
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
        )?;
        directory.write_head(json.end()?, false).map_err(sink_error)
    });
    match published {
        Ok(()) => ResultEnd {
            code,
            status,
            reason,
            detail,
        },
        Err(error) => {
            let failure_code = match error {
                Error::Run(f) => f.code(),
                Error::Stopped => Code::Incomplete,
            };
            // Ordinary final-head filesystem failures contribute 4; allocation
            // and internal failures keep their stronger classification.
            let final_code = code.worst(Code::Incomplete).worst(failure_code);
            let (reason, detail) = if final_code == code {
                (reason, detail)
            } else if final_code == Code::Incomplete {
                (Some("publish_failed"), None)
            } else {
                match error {
                    Error::Run(f) => (Some(f.name()), None),
                    Error::Stopped => (reason, detail),
                }
            };
            ResultEnd {
                code: final_code,
                status: Some("running"),
                reason,
                detail,
            }
        }
    }
}
fn sink_error(e: crate::sink::Error) -> Error {
    match e.class {
        crate::sink::Class::PublishFailed => Error::Stopped,
        crate::sink::Class::HostMemory => Error::memory(),
        crate::sink::Class::InvalidPath => Error::internal(),
        _ => Error::Run(RunFailure::new(crate::attempt::ReasonCode::HostIo)),
    }
}
