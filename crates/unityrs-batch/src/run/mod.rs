//! Temporary B7 local normal-run host. Stops preserve artifacts and bypass
//! normal finishing; B9 owns mandatory lifetime behavior and final ordering.
mod errors;
use crate::{
    control::{Control, Culprit},
    controller::{Boundary, Controller, PreparedSpool},
    execution::Execution,
    finish::{self, Identity, ResultEnd},
    job::RunFailure,
    manifest,
    phase::{self, Account, Buffer},
    phase1::PreparedRun,
    progress::{Phase, live::Sender},
    publish,
    sink::directory::Directory,
    spool::{self, workspace::Workspace},
    start,
    sys::{Lock, Slot},
    winners,
};
pub use errors::Error;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct Stop {
    stopped: AtomicBool,
    failure: Lock<Option<RunFailure>>,
}
impl Control for Stop {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    fn request_stop(&self, failure: RunFailure, _culprit: Culprit) {
        RunFailure::reduce(&mut self.failure.lock(), failure);
        self.stopped.store(true, Ordering::Release);
    }
}
impl Stop {
    fn error(&self, error: phase::Error) -> Error {
        let failure = *self.failure.lock();
        let selected = match error {
            phase::Error::Run(f) => Some(f),
            phase::Error::Stopped => None,
        };
        let mut result = failure;
        if let Some(f) = selected {
            RunFailure::reduce(&mut result, f);
        }
        Error::failure(
            result.unwrap_or_else(|| RunFailure::new(crate::attempt::ReasonCode::Internal)),
        )
    }
}
/// All unavailable capabilities are rejected before host checks or mutation.
pub fn host_check(
    prepared: &PreparedRun,
    bound: u64,
    host: &dyn start::host::Host,
) -> Result<(), Error> {
    start::supported(&prepared.spec).map_err(|_| Error::usage())?;
    let needs =
        start::host::Requirements::new(&prepared.spec, bound).map_err(|e| errors::host(&e))?;
    start::host::check(host, needs).map_err(|e| errors::host(&e))
}
/// The caller owns the progress service and its final acknowledgment. Injectable
/// host adapters keep synthetic acceptance independent of OS-specific probes.
pub fn drive(
    prepared: PreparedRun,
    bound: u64,
    progress: &Sender,
    spool_fs: &dyn start::spool::FileSystem,
    sink_fs: Arc<dyn crate::sink::fs::FileSystem>,
) -> Result<ResultEnd, Error> {
    let local = start::supported(&prepared.spec).map_err(|_| Error::usage())?;
    let control = Arc::new(Stop::default());
    let fin = Account::observed(crate::bound::C_FIN, Some(progress.phase()));
    let started_at = crate::sys::utc_now().map_err(|_| Error::host_io())?;
    let identity = Identity {
        started_at: std::str::from_utf8(&started_at).map_err(|_| Error::internal())?,
        memory_bound: bound,
        spool_peak: 0,
        vm_hwm: 0,
    };
    progress.update(|s| {
        s.phase = Phase::Preflight;
        s.inputs.total = prepared.inputs.len() as u64;
        s.inputs.queued = s.inputs.total;
        s.memory.inflight_limit = prepared.spec.max_inflight_uncompressed_bytes;
        s.spool.size = prepared.spec.spool.size_bytes;
    });
    let mut execution = Execution::prepare(&prepared).map_err(|_| Error::spawn())?;
    let directory = preflight(&local, sink_fs, &fin, &control)?;
    progress.update(|s| s.phase = Phase::Start);
    let started = Arc::new(
        start::spool::start(&local, spool_fs, &|| control.stopped())
            .map_err(|e| errors::spool(&e))?,
    );
    progress.spool(started.counter.clone());
    let fs: Arc<dyn spool::FileSystem> = Arc::new(spool::Host);
    let spool = PreparedSpool::new(
        crate::spool::path::join(&started.run, std::path::Path::new("")).map_err(Error::from)?,
        started.counter.clone(),
        fs.clone(),
    )
    .map_err(|_| Error::host_io())?
    .retained(started.clone(), directory.clone());
    let mut controller = Controller::new(
        prepared,
        spool,
        control.clone(),
        Arc::new(crate::controller::NoHooks),
    )
    .map_err(|_| Error::internal())?;
    controller.progress(progress.clone());
    execution.bind(&controller).map_err(|_| Error::internal())?;
    let account = Arc::new(Account::observed(
        controller.state().spec.max_inflight_uncompressed_bytes,
        Some(progress.phase()),
    ));
    let publish_hooks = Arc::new(publish::NoHooks);
    // Retained pool bookkeeping is reserved before acknowledging the head.
    let owner_slot = Slot::<publish::Owned>::reserve().map_err(|e| control.error(e.into()))?;
    finish::running(controller.state(), &directory, &fin, &identity)
        .map_err(|e| control.error(e))?;
    if controller.drive_local(&mut execution) != Boundary::ReadyForWinners {
        return Err(control.error(phase::Error::Stopped).running());
    }
    let prepared = controller.into_state();
    progress.update(|s| {
        s.phase = Phase::Winners;
        s.memory.slots_reserved = 0;
    });
    let (prepared, workspace, handoff) = select(
        prepared, &started, &account, &control, &execution, &directory, &fs,
    )
    .map_err(Error::running)?;
    let concurrency = prepared.spec.download_concurrency;
    progress.update(|s| {
        s.phase = Phase::Publish;
        s.outputs.winners = handoff.counts.winners;
        s.memory.slots_reserved = 0;
    });
    let outcomes = init_outcomes(&started, handoff.counts.winners, &account, &control)?;
    let done = publication_done(&started.run)?;
    let owned = owner_slot.write(publish::Owned {
        prepared: Lock::new(prepared),
        done,
        workspace,
        handoff,
        directory,
        outcomes,
        account,
        gate: publish::Gate::default(),
        fs,
        control: control.clone(),
        hooks: publish_hooks,
        progress: Some(progress.clone()),
        spool_lease: Some(started.clone()),
    });
    publish::parallel(&owned, &execution, concurrency).map_err(|e| control.error(e).running())?;
    // The last canonical stop check determines the normal snapshot permanently.
    phase::check(&|| control.stopped()).map_err(|e| control.error(e).running())?;
    execution
        .quiesce()
        .map_err(|_| Error::internal().running())?;
    finish_owned(&owned, &fin, &identity, &started, progress, &control)
}
fn finish_owned(
    owned: &publish::Owned,
    fin: &Account,
    identity: &Identity<'_>,
    started: &start::spool::Started,
    progress: &Sender,
    control: &Stop,
) -> Result<ResultEnd, Error> {
    let frozen = owned
        .gate
        .freeze(&owned.outcomes)
        .map_err(|e| control.error(e).running())?;
    progress.update(|s| s.phase = Phase::Finish);
    let prepared = owned.prepared.lock();
    let inputs =
        winners::committed::Committed::received(&owned.done, &prepared.inputs, &*owned.fs, &|| {
            false
        })
        .map_err(|e| control.error(e).running())?;
    let cx = manifest::rows::Context {
        prepared: &prepared,
        inputs: &inputs,
        handoff: &owned.handoff,
        workspace: &owned.workspace,
        frozen: &frozen,
        account: fin,
        stopped: &|| false,
        taken_at: std::time::Instant::now(),
    };
    let ended = crate::sys::utc_now().map_err(|_| Error::host_io().running())?;
    let identity = Identity {
        spool_peak: started.counter.peak(),
        vm_hwm: crate::progress::proc_memory("VmHWM:"),
        started_at: identity.started_at,
        memory_bound: identity.memory_bound,
    };
    let result = finish::normal(
        &cx,
        &owned.directory,
        &identity,
        std::str::from_utf8(&ended).map_err(|_| Error::internal().running())?,
    )
    .map_err(|e| control.error(e).running())?;
    progress.update(|s| {
        s.inputs.complete = prepared
            .inputs
            .iter()
            .filter(|s| {
                s.failure_bundle.is_none()
                    && s.received
                        .is_some_and(|r| r.state == crate::attempt::State::Done)
            })
            .count() as u64;
        s.inputs.failed = s.inputs.total - s.inputs.complete;
        s.inputs.running = 0;
        s.inputs.queued = 0;
    });
    Ok(result)
}
fn select(
    prepared: PreparedRun,
    started: &Arc<start::spool::Started>,
    account: &Arc<Account>,
    control: &Arc<Stop>,
    execution: &Execution,
    directory: &Arc<Directory>,
    fs: &Arc<dyn spool::FileSystem>,
) -> Result<(PreparedRun, Workspace, winners::Handoff), Error> {
    let fs = fs.clone();
    let path = crate::spool::path::join(&started.run, std::path::Path::new("winners/workspace"))
        .map_err(Error::from)?;
    let mut workspace =
        Workspace::create(path, started.counter.clone(), fs.clone()).map_err(Error::from)?;
    let done = crate::spool::path::join(&started.run, std::path::Path::new("done"))
        .map_err(Error::from)?;
    let account = account.clone();
    let stop = control.clone();
    let lease = (started.clone(), directory.clone());
    let task = execution
        .spawn(move || {
            let _lease = lease;
            (|| {
                let inputs = winners::committed::Committed::received(
                    &done,
                    &prepared.inputs,
                    &*fs,
                    &|| stop.stopped(),
                )?;
                let handoff = winners::complete(&mut winners::Context {
                    spec: &prepared.spec,
                    prepared: &prepared.winners,
                    inputs: &inputs,
                    workspace: &mut workspace,
                    account: &account,
                    options: crate::sort::Options::default(),
                    stopped: &|| stop.stopped(),
                    hooks: &crate::sort::NoHooks,
                    observer: &winners::NoObserver,
                })?;
                Ok::<_, phase::Error>((prepared, workspace, handoff))
            })()
        })
        .map_err(|e| control.error(e.into()))?;
    while !task.is_finished() {
        phase::check(&|| control.stopped()).map_err(|e| control.error(e))?;
        execution.wait().map_err(|_| Error::host_io())?;
    }
    phase::check(&|| control.stopped()).map_err(|e| control.error(e))?;
    let result = execution
        .finish(&task)
        .map_err(|()| Error::internal())?
        .map_err(|e| control.error(e))?;
    phase::check(&|| control.stopped()).map_err(|e| control.error(e))?;
    Ok(result)
}

fn preflight(
    local: &start::Local<'_>,
    fs: Arc<dyn crate::sink::fs::FileSystem>,
    fin: &Account,
    control: &Stop,
) -> Result<Arc<Directory>, Error> {
    let mut scratch = Buffer::<u8>::new(fin.meter(), manifest::head::HEAD_LIMIT + 1)
        .map_err(|e| control.error(e))?;
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

fn init_outcomes(
    started: &start::spool::Started,
    winners: u64,
    account: &Account,
    control: &Stop,
) -> Result<crate::outcomes::Outcomes, Error> {
    let mut zeros =
        Buffer::<u8>::new(account.meter(), 64 * 1024).map_err(|e| control.error(e).running())?;
    zeros.fill(0);
    crate::outcomes::Outcomes::create(
        &started.run,
        winners,
        0,
        &started.counter,
        &spool::Host,
        &zeros,
        &|| control.stopped(),
    )
    .map_err(|e| control.error(e).running())
}

fn publication_done(run: &std::path::Path) -> Result<std::path::PathBuf, Error> {
    crate::spool::path::join(run, std::path::Path::new("done"))
        .map_err(|e| Error::from(e).running())
}

#[cfg(test)]
mod tests;
