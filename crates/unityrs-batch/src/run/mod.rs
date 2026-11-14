//! The library run: the controller's body through hand-over, then the
//! finisher, on the calling thread. Every end after the controller exists
//! goes through one [`HandOver`] and [`finish::run`]; the owners stay parked
//! in [`Parked`] until the finisher has returned, as the process host parks
//! them until `_exit`. Cooperative quiescence is for tests alone.
mod errors;
mod phases;
use crate::{
    control::{Control, Culprit},
    controller::{self, Boundary, Controller, PreparedSpool},
    execution::Execution,
    finish::{self, FinishCause, HandOver, Identity, ResultEnd, Selected},
    job::RunFailure,
    lifetime::{State, Stop},
    phase::{self, Account},
    phase1::{Owners, PreparedRun},
    progress::{Phase, live::Sender},
    publish, spool, start,
    sys::{Shared, Slot, Wake},
};
pub use errors::Error;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

/// The library's stop word: the process word's reduction and take, and a
/// wake of the Execution's controller wait on every request.
#[derive(Default)]
pub struct Lifetime {
    pub state: State,
    wake: OnceLock<Arc<Wake>>,
}
impl Lifetime {
    /// An external stop (a signal, the deadline): what the host's handlers
    /// and lifecycle thread request.
    pub fn request(&self, stop: Stop) {
        self.state.request(stop);
        if let Some(wake) = self.wake.get() {
            let _ = wake.notify();
        }
    }
}
impl Control for Lifetime {
    fn stopped(&self) -> bool {
        self.state.live().is_some()
    }
    fn request_stop(&self, failure: RunFailure, culprit: Culprit) {
        self.request((failure, culprit).into());
    }
}
/// Observation points around the hand-over, for acceptance. They run on the
/// controller's thread and must not drop anything they are shown.
pub trait Observer: Send + Sync {
    /// Just before a library step that has no hook of its own.
    fn seam(&self, _: Seam) {}
    fn handed_over(&self, _: &HandOver) {}
    fn taken(&self, _: Option<Stop>) {}
    /// After the finisher, while the parked owners are still alive.
    fn finished(&self, _: &finish::Finished) {}
}
/// The library body's own seams, between the controller's and publication's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seam {
    /// Before phase 6's pool task is spawned.
    Selection,
    /// Before the outcome files are created.
    Outcomes,
    /// After publication's drain and its final audit, before the last check.
    Verified,
}
pub struct NoObserver;
impl Observer for NoObserver {}
pub struct Options {
    pub lifetime: Arc<Lifetime>,
    pub controller_hooks: Arc<dyn controller::Hooks>,
    pub publish_hooks: Arc<dyn publish::Hooks + Send>,
    pub observer: Arc<dyn Observer>,
    /// Counts the drop of the parked owners, which follows the finisher.
    pub parked_drops: Option<Arc<AtomicUsize>>,
    /// The spool's filesystem after startup; the host's when `None`.
    pub spool: Option<Arc<dyn spool::FileSystem>>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            lifetime: Arc::new(Lifetime::default()),
            controller_hooks: Arc::new(controller::NoHooks),
            publish_hooks: Arc::new(publish::NoHooks),
            observer: Arc::new(NoObserver),
            parked_drops: None,
            spool: None,
        }
    }
}
/// What the controller keeps past its hand-over: its Execution (and so the
/// pool's runtime), its slots and residue, publication's owner. Dropped
/// only once the finisher has returned.
#[derive(Default)]
struct Parked {
    execution: Option<Execution>,
    owners: Option<Owners>,
    residue: Option<controller::Residue>,
    owned: Option<Shared<publish::Owned>>,
    drops: Option<Arc<AtomicUsize>>,
}
impl Drop for Parked {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            drops.fetch_add(1, Ordering::SeqCst);
        }
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
/// Preflight's read of the UnityCN key a version 2 spec names
/// (`docs/BATCH_SPEC.md` 1.1, `start::key`) from `env`, the process's
/// environment in the binary: the key is set in every load's options of
/// `prepared`, before [`drive`], which refuses a spec naming a key that was
/// not read. A spec naming none reads nothing. The 69's detail is the
/// variable's name, never its value.
pub fn unitycn_key(
    prepared: &mut PreparedRun,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<(), Error> {
    let Some(name) = prepared.spec.unitycn_key.as_deref() else {
        return Ok(());
    };
    let key = start::key::read(name, env).map_err(|reason| Error::preflight(reason, name))?;
    Arc::get_mut(&mut prepared.memory)
        .ok_or(())
        .and_then(|plan| plan.install_key(key))
        .map_err(|()| Error::internal())
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
    drive_with(
        prepared,
        bound,
        progress,
        (spool_fs, sink_fs),
        &Options::default(),
    )
}
/// [`drive`] with publication observation points, for counted acceptance.
#[cfg(any(test, feature = "test-hooks"))]
pub fn drive_hooked(
    prepared: PreparedRun,
    bound: u64,
    progress: &Sender,
    fs: (
        &dyn start::spool::FileSystem,
        Arc<dyn crate::sink::fs::FileSystem>,
    ),
    publish_hooks: Arc<dyn publish::Hooks + Send>,
) -> Result<ResultEnd, Error> {
    drive_with(
        prepared,
        bound,
        progress,
        fs,
        &Options {
            publish_hooks,
            ..Options::default()
        },
    )
}
/// [`drive`] with its stop word, hooks and observer supplied.
pub fn drive_with(
    prepared: PreparedRun,
    bound: u64,
    progress: &Sender,
    (spool_fs, sink_fs): (
        &dyn start::spool::FileSystem,
        Arc<dyn crate::sink::fs::FileSystem>,
    ),
    options: &Options,
) -> Result<ResultEnd, Error> {
    start::supported(&prepared.spec).map_err(|_| Error::usage())?;
    let fin = Account::observed(crate::bound::C_FIN, Some(progress.phase()));
    let started_at = crate::sys::utc_now().map_err(|_| Error::host_io())?;
    let started_at = std::str::from_utf8(&started_at).map_err(|_| Error::internal())?;
    let mut parked = Parked::default();
    parked.drops.clone_from(&options.parked_drops);
    let body = Body {
        bound,
        progress,
        spool_fs,
        sink_fs,
        options,
        fin: &fin,
        started_at,
    };
    let handover = body
        .run(prepared, &mut parked)
        .map_err(|e| terminal(e, options.lifetime.state.take()))?;
    options.observer.handed_over(&handover);
    let taken = options.lifetime.state.take();
    options.observer.taken(taken);
    progress.update(|s| s.phase = Phase::Finish);
    let started = handover.spool.as_ref().and_then(|s| s.lease.clone());
    let identity = Identity {
        started_at,
        memory_bound: bound,
        spool_peak: started.as_ref().map_or(0, |s| s.counter.peak()),
        vm_hwm: crate::progress::proc_memory("VmHWM:"),
    };
    let finished = finish::run(
        &handover,
        taken,
        &mut finish::Finisher {
            account: &fin,
            identity: &identity,
            // `T_hard` read live: unarmed until a deadline is armed.
            budget: finish::budget::Budget {
                hard: &options.lifetime.state,
                reconcile: false,
            },
            clock: &finish::budget::BootClock,
            backend: &mut finish::budget::NoJournal,
        },
    );
    phases::post_finish(progress, &handover);
    options.observer.finished(&finished);
    drop(handover);
    // The owners outlive the finisher, as the process host's do until _exit.
    drop(parked);
    Ok(finished.end)
}
/// A preflight end's code against the stop the finisher took (9.1).
fn terminal(mut error: Error, taken: Option<Stop>) -> Error {
    let code = finish::reduce(error.code, taken);
    if let Some(stop) = taken.filter(|_| code != error.code) {
        error = Error::failure_stop(stop);
    }
    error
}
struct Body<'a> {
    bound: u64,
    progress: &'a Sender,
    spool_fs: &'a dyn start::spool::FileSystem,
    sink_fs: Arc<dyn crate::sink::fs::FileSystem>,
    options: &'a Options,
    fin: &'a Account,
    started_at: &'a str,
}
impl Body<'_> {
    /// The controller's body. `Err` is a terminal end before the `running`
    /// head; every later end is a hand-over.
    #[allow(clippy::too_many_lines)] // 2.11's order, start to hand-over, in one place.
    fn run(&self, prepared: PreparedRun, parked: &mut Parked) -> Result<HandOver, Error> {
        let local = start::supported(&prepared.spec).map_err(|_| Error::usage())?;
        // The host reads a named key at preflight ([`unitycn_key`]).
        if local.spec().unitycn_key.is_some() && prepared.memory.key.is_none() {
            return Err(Error::internal());
        }
        let control = self.options.lifetime.clone();
        let progress = self.progress;
        progress.update(|s| {
            s.phase = Phase::Preflight;
            s.inputs.total = prepared.inputs.len() as u64;
            s.inputs.queued = s.inputs.total;
            s.memory.inflight_limit = prepared.spec.max_inflight_uncompressed_bytes;
            s.spool.size = prepared.spec.spool.size_bytes;
        });
        let execution = Execution::prepare(&prepared).map_err(|_| Error::spawn())?;
        let _ = control.wake.set(execution.wake());
        let execution = parked.execution.insert(execution);
        let directory = phases::preflight(&local, self.sink_fs.clone(), self.fin)?;
        progress.update(|s| s.phase = Phase::Start);
        let started = match start::spool::start(&local, self.spool_fs, &|| control.stopped()) {
            Ok(started) => Arc::new(started),
            // A stop before or during the spool's startup or cleanup is
            // 7.4's first row, decided by the finisher like any later stop.
            Err(e) if e.class == start::spool::Class::Stopped => {
                let mut handover = HandOver::new(FinishCause::Stop, Phase::Start);
                handover.directory = Some(directory);
                return Ok(handover);
            }
            Err(e) => return Err(errors::spool(&e)),
        };
        progress.spool(started.counter.clone());
        let fs = match &self.options.spool {
            Some(fs) => fs.clone(),
            None => Arc::new(spool::Host) as Arc<dyn spool::FileSystem>,
        };
        let spool = PreparedSpool::new(
            crate::spool::path::join(&started.run, std::path::Path::new(""))
                .map_err(Error::from)?,
            started.counter.clone(),
            fs.clone(),
        )
        .map_err(|_| Error::host_io())?
        .retained(started.clone(), directory.clone());
        let mut controller = Controller::new(
            prepared,
            spool,
            control.clone(),
            self.options.controller_hooks.clone(),
        )
        .map_err(|_| Error::internal())?;
        controller.progress(progress.clone());
        execution.bind(&controller).map_err(|_| Error::internal())?;
        let account = Arc::new(Account::observed(
            controller.state().spec.max_inflight_uncompressed_bytes,
            Some(progress.phase()),
        ));
        // Retained bookkeeping is reserved before acknowledging the head.
        let owner_slot = Slot::<publish::Owned>::reserve().map_err(|_| Error::memory())?;
        let facts_slot = Slot::reserve().map_err(|_| Error::memory())?;
        let done = phases::done(&started.run)?;
        let mut handover = HandOver::new(FinishCause::Stop, Phase::Start);
        handover.directory = Some(directory.clone());
        handover.spool = Some(finish::Spool {
            run: started.run.clone(),
            done: done.clone(),
            fs: fs.clone(),
            lease: Some(started.clone()),
        });
        if control.stopped() {
            // 7.4's first row: the `running` write was never sent.
            return Ok(handover);
        }
        finish::running(
            controller.state(),
            &directory,
            self.fin,
            &Identity {
                started_at: self.started_at,
                memory_bound: self.bound,
                spool_peak: 0,
                vm_hwm: 0,
            },
        )
        .map_err(errors::running)?;
        handover.running = true;
        let boundary = controller.drive_local(execution);
        let reached = controller.reached();
        let pending = controller.pending_commit();
        let (prepared, residue) = controller.into_parts();
        let (facts, owners) = prepared.split();
        parked.owners = Some(owners);
        parked.residue = Some(residue);
        let facts = facts_slot.write(facts);
        handover.facts = Some(facts.clone());
        handover.pending = pending;
        if boundary != Boundary::ReadyForWinners {
            return Ok(stamp(handover, reached));
        }
        progress.update(|s| {
            s.phase = Phase::Winners;
            s.memory.slots_reserved = 0;
        });
        self.options.observer.seam(Seam::Selection);
        let Some((workspace, handoff)) = phases::select(
            &facts, &started, &account, &control, execution, &directory, &fs,
        ) else {
            return Ok(stamp(handover, Phase::Winners));
        };
        let concurrency = facts.spec.download_concurrency;
        progress.update(|s| {
            s.phase = Phase::Publish;
            s.outputs.winners = handoff.counts.winners;
            s.memory.slots_reserved = 0;
        });
        let archives = u64::try_from(facts.spec.archives.len()).map_err(|_| Error::internal())?;
        self.options.observer.seam(Seam::Outcomes);
        let Some(outcomes) = phases::outcomes(
            &started,
            (handoff.counts.winners, archives),
            &account,
            (&*control, &*fs),
        ) else {
            handover.selected = Some(Selected::Parts { workspace, handoff });
            return Ok(stamp(handover, Phase::Publish));
        };
        let owned = owner_slot.write(publish::Owned {
            facts,
            done,
            workspace,
            handoff,
            directory,
            outcomes,
            account,
            gate: publish::Gate::default(),
            fs,
            control: control.clone(),
            hooks: self.options.publish_hooks.clone(),
            progress: Some(progress.clone()),
            spool_lease: Some(started.clone()),
        });
        parked.owned = Some(owned.clone());
        let published = publish::parallel(&owned, execution, concurrency);
        if published.is_ok() {
            self.options.observer.seam(Seam::Verified);
        }
        // The last canonical stop check determines the normal snapshot.
        let ended = published.and_then(|_| phase::check(&|| control.stopped()));
        if let Err(error) = ended {
            phases::halt(&*control, error);
        } else {
            handover.cause = FinishCause::Normal;
        }
        // The hand-over takes the commit lock: a commit under way lands
        // first, and none after (W15).
        owned.gate.close();
        handover.selected = Some(Selected::Owned(owned));
        Ok(stamp(handover, Phase::Publish))
    }
}
/// The phase reached and the hand-over instant, sampled as it is made.
fn stamp(mut handover: HandOver, phase: Phase) -> HandOver {
    handover.phase = phase;
    handover.at = std::time::Instant::now();
    handover.boot = crate::lifetime::Boot::now().ok();
    handover
}

#[cfg(test)]
mod tests;
