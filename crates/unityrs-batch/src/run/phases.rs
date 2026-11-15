//! The library body's steps around the controller: preflight, phase 6's
//! pool task, phase 7's outcomes, and the finish's progress. A step that
//! ends on a stop or a run failure returns `None` after requesting the stop;
//! its owners (a held task's captures among them) stay where they are.
use super::{Error, errors};
use crate::{
    control::{Control, Culprit},
    execution::Execution,
    finish::HandOver,
    job::RunFailure,
    manifest,
    phase::{self, Account, Buffer},
    phase1::Facts,
    progress::live::Sender,
    sink::directory::Directory,
    spool::{self, workspace::Workspace},
    start,
    sys::Shared,
    winners,
};
use std::sync::Arc;

pub(super) fn preflight(
    local: &start::Local<'_>,
    fs: Arc<dyn crate::sink::fs::FileSystem>,
    fin: &Account,
) -> Result<Arc<Directory>, Error> {
    let mut scratch =
        Buffer::<u8>::new(fin.meter(), manifest::head::HEAD_LIMIT + 1).map_err(errors::running)?;
    scratch.fill(0);
    Ok(Arc::new(
        Directory::preflight(
            local.root(),
            &local.spec().run_id,
            local.spec().attempt,
            fs,
            &mut scratch,
        )
        .map_err(errors::sink)?,
    ))
}
/// Every end after the head is a stop: a run failure requests its own,
/// with no culprit, before the hand-over; a stop already holds one.
pub(super) fn halt(control: &dyn Control, error: phase::Error) {
    match error {
        phase::Error::Run(failure) => control.request_stop(failure, Culprit::None),
        phase::Error::Stopped if !control.stopped() => control.request_stop(
            RunFailure::new(crate::attempt::ReasonCode::Internal),
            Culprit::None,
        ),
        phase::Error::Stopped => {}
    }
}
/// Phase 6 on one pool task, holding a share of the frozen facts and its
/// own workspace. On a stop the task keeps both; the controller's share is
/// the finisher's, so a held task blocks no read.
pub(super) fn select(
    facts: &Shared<Facts>,
    started: &Arc<start::spool::Started>,
    account: &Arc<Account>,
    control: &Arc<super::Lifetime>,
    execution: &Execution,
    directory: &crate::sink::Sink,
    fs: &Arc<dyn spool::FileSystem>,
) -> Option<(Workspace, winners::Handoff)> {
    let result = (|| -> Result<_, phase::Error> {
        let path =
            crate::spool::path::join(&started.run, std::path::Path::new("winners/workspace"))?;
        let mut workspace = Workspace::create(path, started.counter.clone(), fs.clone())?;
        let done = crate::spool::path::join(&started.run, std::path::Path::new("done"))?;
        let (facts, fs, account, stop) =
            (facts.clone(), fs.clone(), account.clone(), control.clone());
        let lease = (started.clone(), directory.clone());
        let task = execution.spawn_named("a phase-6 task", move || {
            let _lease = lease;
            (|| {
                let inputs =
                    winners::committed::Committed::received(&done, &facts.inputs, &*fs, &|| {
                        stop.stopped()
                    })?;
                let handoff = winners::complete(&mut winners::Context {
                    spec: &facts.spec,
                    prepared: &facts.winners,
                    inputs: &inputs,
                    workspace: &mut workspace,
                    account: &account,
                    options: crate::sort::Options::default(),
                    stopped: &|| stop.stopped(),
                    hooks: &crate::sort::NoHooks,
                    observer: &winners::NoObserver,
                })?;
                Ok::<_, phase::Error>((workspace, handoff))
            })()
        })?;
        while !task.is_finished() {
            phase::check(&|| control.stopped())?;
            execution.wait()?;
        }
        phase::check(&|| control.stopped())?;
        let result = execution
            .finish(&task)
            .map_err(|()| phase::Error::internal())??;
        phase::check(&|| control.stopped())?;
        Ok(result)
    })();
    result.map_err(|e| halt(&**control, e)).ok()
}
pub(super) fn outcomes(
    started: &start::spool::Started,
    (winners, archives): (u64, u64),
    account: &Account,
    (control, fs): (&dyn Control, &dyn spool::FileSystem),
) -> Option<crate::outcomes::Outcomes> {
    let result = (|| -> Result<_, phase::Error> {
        let mut zeros = Buffer::<u8>::new(account.meter(), 64 * 1024)?;
        zeros.fill(0);
        crate::outcomes::Outcomes::create(
            &started.run,
            winners,
            archives,
            &started.counter,
            fs,
            &zeros,
            &|| control.stopped(),
        )
    })();
    result.map_err(|e| halt(control, e)).ok()
}
/// Reserved before the head, with the other retained paths.
pub(super) fn done(run: &std::path::Path) -> Result<std::path::PathBuf, Error> {
    crate::spool::path::join(run, std::path::Path::new("done")).map_err(Error::from)
}
/// The finish's counts (`BATCH_SPEC.md` section 10), from the frozen
/// facts, with `found`, the hand-over slot's input whose commit the
/// finisher's stat found, counted as committed. Nothing is queued once a
/// stop is seen. A dispatched input that did not commit counts in
/// `running` in the finisher's lines and, at the `last` (the `exit`
/// line's), in `not_run`; every other input not committed is `not_run`.
/// Without facts (a stop before `running`), whatever was running or
/// queued is `not_run`, so the counts still partition `total`.
pub(super) fn post_finish(progress: &Sender, handover: &HandOver, found: Option<u32>, last: bool) {
    progress.update(|s| finish_counts(s, handover, found, last));
}
/// [`post_finish`]'s counts on `s`: the `exit` line's alone, never posted
/// as a line of their own, at the `last`.
pub(super) fn finish_counts(
    s: &mut crate::progress::Snapshot,
    handover: &HandOver,
    found: Option<u32>,
    last: bool,
) {
    let Some(facts) = handover.facts.as_deref() else {
        s.inputs.running = 0;
        s.inputs.queued = 0;
        s.inputs.not_run = s.inputs.total - s.inputs.complete - s.inputs.failed;
        return;
    };
    {
        let committed = |(i, state): &(usize, &crate::phase1::InputState)| {
            state.phase == crate::phase1::InputPhase::Committed
                || found.is_some_and(|f| usize::try_from(f).is_ok_and(|f| f == *i))
        };
        s.inputs.complete = facts
            .inputs
            .iter()
            .enumerate()
            .filter(|entry| {
                committed(entry)
                    && entry.1.failure_bundle.is_none()
                    && entry
                        .1
                        .received
                        .is_some_and(|r| r.state == crate::attempt::State::Done)
            })
            .count() as u64;
        s.inputs.failed =
            facts.inputs.iter().enumerate().filter(committed).count() as u64 - s.inputs.complete;
        s.inputs.running = if last {
            0
        } else {
            facts
                .inputs
                .iter()
                .enumerate()
                .filter(|entry| {
                    entry.1.phase == crate::phase1::InputPhase::Running && !committed(entry)
                })
                .count() as u64
        };
        s.inputs.not_run = s.inputs.total - s.inputs.complete - s.inputs.failed - s.inputs.running;
        s.inputs.queued = 0;
    }
}
