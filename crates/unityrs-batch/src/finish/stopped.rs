//! The finisher's one entry, from a hand-over and the stop it took. Every
//! read is of frozen facts, the commit-free `done/` set, sealed phase-6
//! artifacts and outcome files the closed gate froze; nothing it does can
//! wait on a controller owner.
use super::{
    Ending, FinishCause, HandOver, Identity, ResultEnd, Selected,
    budget::{self, Backend, Budget, Budgeted, Clock, Settled},
    conclude, normal_into, reduce,
};
use crate::{
    attempt::ReasonCode,
    job::RunFailure,
    lifetime::{SpoolDetail, Stop},
    manifest, outcomes,
    phase::Account,
    phase::Error,
    phase1::Facts,
    progress::Phase,
    reason::{Code, StopReason},
    winners::committed::Committed,
};

pub struct Finisher<'a> {
    /// The finisher's own fixed storage (`bound::C_FIN`).
    pub account: &'a Account,
    pub identity: &'a Identity<'a>,
    pub budget: Budget<'a>,
    pub clock: &'a dyn Clock,
    pub backend: &'a mut dyn Backend,
}
/// The finisher's one stat of the hand-over slot's `done/<input>/` (2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    /// Its rename came before the hand-over: committed, whatever its bit.
    Found(u32),
    /// `ENOENT`: uncommitted, written `stopped`.
    Absent(u32),
    /// Any other error: uncommitted, the conservative answer, with a
    /// `commit unknown at take` trace line and a stderr warning.
    Unknown(u32, Option<i32>),
}
pub struct Finished {
    pub end: ResultEnd,
    pub reconcile: Option<Reconcile>,
    pub settled: Settled,
}
/// Run the finisher once. `taken` is the stop it took (`State::take`): a
/// normal end ignores it; a stop's head names it; a terminal code yields
/// only to a more severe one.
pub fn run(h: &HandOver, taken: Option<Stop>, fin: &mut Finisher<'_>) -> Finished {
    let mut finished = Finished {
        end: ResultEnd {
            code: Code::Internal,
            status: None,
            reason: Some("internal"),
            detail: None,
        },
        reconcile: None,
        settled: Settled::default(),
    };
    // Steps 1 and 2 do nothing when the spool's lock was never taken.
    if h.spool.is_some() {
        finished.settled = budget::settle(fin.backend, fin.budget, fin.clock);
    }
    match h.cause {
        FinishCause::Terminal(t) => {
            let code = reduce(t.code, taken);
            // The terminal's reason keeps a code a stop only equals.
            finished.end = no_head(code, taken.filter(|_| code != t.code), t.reason);
        }
        FinishCause::Stop => {
            let stop = taken.unwrap_or(Stop::external(StopReason::Internal));
            finished.end = if h.running {
                // Whatever stops the rows leaves `running`, never no head.
                stopped(h, stop, fin, &mut finished.reconcile)
                    .unwrap_or_else(|failure| unwritten(stop, failure))
            } else {
                // 7.4's first row: no head, the stop's code.
                no_head(stop.reason.code(), Some(stop), "internal")
            };
        }
        // A normal end always follows `running`; one that cannot read its
        // rows leaves that head, with the failure's code.
        FinishCause::Normal if h.running => {
            finished.end = normal(h, fin).unwrap_or_else(|failure| ResultEnd {
                code: failure.code(),
                status: Some("running"),
                reason: Some(failure.name()),
                detail: None,
            });
        }
        FinishCause::Normal => {}
    }
    finished
}
fn failure(error: Error) -> RunFailure {
    match error {
        Error::Run(f) => f,
        Error::Stopped => internal(),
    }
}
fn internal() -> RunFailure {
    RunFailure::new(ReasonCode::Internal)
}
fn no_head(code: Code, stop: Option<Stop>, reason: &'static str) -> ResultEnd {
    ResultEnd {
        code,
        status: None,
        reason: Some(stop.map_or(reason, |s| s.reason.name())),
        detail: stop.and_then(|s| s.detail).map(SpoolDetail::name),
    }
}
fn ended_at(buffer: &mut [u8; 20]) -> Option<&str> {
    *buffer = crate::sys::utc_now().ok()?;
    std::str::from_utf8(buffer).ok()
}
fn normal(h: &HandOver, fin: &mut Finisher<'_>) -> Result<ResultEnd, RunFailure> {
    let Some(Selected::Owned(owned)) = &h.selected else {
        return Err(internal());
    };
    let facts = h.facts.as_deref().ok_or_else(internal)?;
    let directory = h.directory.as_deref().ok_or_else(internal)?;
    let inputs =
        Committed::received(&owned.done, &facts.inputs, &*owned.fs, &|| false).map_err(failure)?;
    let cx = manifest::rows::Context {
        facts,
        inputs: &inputs,
        selected: Some(manifest::rows::Selected {
            handoff: &owned.handoff,
            workspace: &owned.workspace,
            outcomes: &owned.outcomes,
        }),
        stop: None,
        found: None,
        account: fin.account,
        stopped: &|| false,
        taken_at: h.at,
    };
    let mut rows = rows(facts, directory);
    let mut time = [0; 20];
    Ok(normal_into(
        &cx,
        &mut Budgeted {
            rows: &mut rows,
            budget: fin.budget,
            clock: fin.clock,
            cut: false,
        },
        directory,
        fin.identity,
        ended_at(&mut time),
    ))
}
fn rows<'a>(
    facts: &'a Facts,
    directory: &'a crate::sink::directory::Directory,
) -> manifest::chunks::Directory<'a> {
    manifest::chunks::Directory {
        directory,
        run_id: &facts.spec.run_id,
        attempt: facts.spec.attempt,
    }
}
/// 7.4's rows for a stop after the `running` head: the received families,
/// and in phase 7 every family, statuses from the outcomes as frozen. A
/// failure to read what the rows need (`Err`) writes nothing more: the head
/// stays `running`, and the failure's code meets the stop's.
fn stopped(
    h: &HandOver,
    stop: Stop,
    fin: &mut Finisher<'_>,
    reconcile: &mut Option<Reconcile>,
) -> Result<ResultEnd, RunFailure> {
    let facts = h.facts.as_deref().ok_or_else(internal)?;
    let directory = h.directory.as_deref().ok_or_else(internal)?;
    let spool = h.spool.as_ref().ok_or_else(internal)?;
    if let Some(input) = h.pending {
        *reconcile = Some(Reconcile::Unknown(input, None));
        *reconcile = Some(one_stat(spool, input)?);
    }
    let found = match *reconcile {
        Some(Reconcile::Found(input)) => Some(input),
        _ => None,
    };
    let inputs = Committed::received(&spool.done, &facts.inputs, &*spool.fs, &|| false)
        .map_err(failure)?
        .with_found(found);
    let view;
    let selected = match h.selected.as_ref().filter(|_| h.phase == Phase::Publish) {
        None => None,
        Some(sel) => {
            let outcomes: &dyn outcomes::Read = match sel {
                Selected::Owned(owned) => &owned.outcomes,
                Selected::Parts { handoff, .. } => {
                    let archives =
                        u64::try_from(facts.spec.archives.len()).map_err(|_| internal())?;
                    view = outcomes::View::open(
                        &spool.run,
                        handoff.counts.winners,
                        archives,
                        &*spool.fs,
                    )
                    .map_err(failure)?;
                    &view
                }
            };
            Some(manifest::rows::Selected {
                handoff: sel.handoff(),
                workspace: sel.workspace(),
                outcomes,
            })
        }
    };
    let cx = manifest::rows::Context {
        facts,
        inputs: &inputs,
        selected,
        stop: Some(stop),
        found,
        account: fin.account,
        stopped: &|| false,
        taken_at: h.at,
    };
    let mut rows = rows(facts, directory);
    let mut time = [0; 20];
    Ok(conclude(
        &cx,
        &mut Budgeted {
            rows: &mut rows,
            budget: fin.budget,
            clock: fin.clock,
            cut: false,
        },
        directory,
        fin.identity,
        ended_at(&mut time),
        Ending {
            code: stop.reason.code(),
            failure: None,
            stop: Some(stop),
            phase: h.phase,
        },
    ))
}
/// No rows or head after `running`: the more severe of the stop's code and
/// the failure's, named by whichever set it.
pub(super) fn unwritten(stop: Stop, failure: RunFailure) -> ResultEnd {
    let code = stop.reason.code().worst(failure.code());
    let by_stop = code == stop.reason.code();
    ResultEnd {
        code,
        status: Some("running"),
        reason: Some(if by_stop {
            stop.reason.name()
        } else {
            failure.name()
        }),
        detail: stop.detail.filter(|_| by_stop).map(SpoolDetail::name),
    }
}
/// The stat itself never fails the finish: any errno, `ENOMEM` included,
/// is the conservative answer. Only a refused path allocation, ours or the
/// stat's C string (an `OutOfMemory` with no errno), is a failure
/// (`host_memory`).
fn one_stat(spool: &super::Spool, input: u32) -> Result<Reconcile, RunFailure> {
    let path = crate::spool::path::indexed(&spool.done, "", u64::from(input))
        .map_err(|_| RunFailure::new(ReasonCode::HostMemory))?;
    Ok(match spool.fs.stat_directory(&path) {
        Ok(()) => Reconcile::Found(input),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Reconcile::Absent(input),
        // Our own refused path copy, not the stat's answer.
        Err(e) if e.kind() == std::io::ErrorKind::OutOfMemory && e.raw_os_error().is_none() => {
            return Err(RunFailure::new(ReasonCode::HostMemory));
        }
        Err(e) => Reconcile::Unknown(input, e.raw_os_error()),
    })
}
