//! The library run: the controller's body through hand-over, then the
//! finisher, on the calling thread. Every end after the controller exists
//! goes through one [`HandOver`] and [`finish::run`]; the owners stay parked
//! in [`Parked`] until the finisher has returned, as the process host parks
//! them until `_exit`. Cooperative quiescence is for tests alone.
mod errors;
#[cfg(feature = "test-hooks")]
pub mod faults;
#[cfg(feature = "test-hooks")]
pub mod hooks;
mod network;
mod phases;
pub mod process;
mod store;
use crate::{
    control::{Control, Culprit},
    controller::{self, Boundary, Controller, PreparedSpool},
    execution::Execution,
    finish::{self, FinishCause, HandOver, Identity, ResultEnd, Selected},
    job::RunFailure,
    lifetime::{Process, State, Stop},
    phase::{self, Account},
    phase1::{Owners, PreparedRun},
    progress::{Phase, live::Sender},
    publish, spool, start,
    sys::{Shared, Slot, Wake},
};
pub use errors::Error;
#[cfg(any(test, feature = "test-hooks"))]
pub use network::Hooks;
pub use network::Network;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

/// The run's stop word: the library's own, or the process host's, whose
/// request also writes the lifecycle's self-pipe. Each request wakes the
/// Execution's controller wait.
#[derive(Default)]
pub struct Lifetime {
    process: Option<&'static Process>,
    own: State,
    wake: OnceLock<Arc<Wake>>,
}
impl Lifetime {
    /// The process host's: its handlers and lifecycle share this word.
    #[must_use]
    pub fn process(process: &'static Process) -> Self {
        Self {
            process: Some(process),
            own: State::new(),
            wake: OnceLock::new(),
        }
    }
    #[must_use]
    pub fn state(&self) -> &State {
        self.process.map_or(&self.own, |p| &p.state)
    }
    /// An external stop (a signal, the deadline): what the host's handlers
    /// and lifecycle thread request.
    pub fn request(&self, stop: Stop) {
        match self.process {
            Some(process) => process.request(stop),
            None => self.own.request(stop),
        }
        if let Some(wake) = self.wake.get() {
            let _ = wake.notify();
        }
    }
}
impl Control for Lifetime {
    fn stopped(&self) -> bool {
        self.state().live().is_some()
    }
    fn request_stop(&self, failure: RunFailure, culprit: Culprit) {
        self.request((failure, culprit).into());
    }
}
/// Observation points around the hand-over, for acceptance. They run on the
/// controller's thread and must not drop anything they are shown.
pub trait Observer: Send + Sync {
    #[cfg(feature = "test-hooks")]
    fn sort_options(&self) -> crate::sort::Options {
        crate::sort::Options::default()
    }
    /// Just before a library step that has no hook of its own.
    fn seam(&self, _: Seam) {}
    fn handed_over(&self, _: &HandOver) {}
    fn taken(&self, _: Option<Stop>) {}
    /// After the finisher, while the parked owners are still alive.
    fn finished(&self, _: &finish::Finished) {}
    /// Test-only teardown schedule. Completed workers and stage owners retire
    /// first; the callback can drive the still-live reactor before the real
    /// registered wakes and runtime are dropped. Production never installs it.
    #[cfg(feature = "test-hooks")]
    fn test_before_wake_drop(&self) -> Option<fn(&tokio::runtime::Runtime)> {
        None
    }
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
    /// Stages URL bundles; without it a URL is refused.
    pub network: Option<Network>,
    /// The process warnings both heads record, the `running` head's and
    /// the final one's, each sampled as it is encoded (7.3): the process
    /// host's live counters; a library run has none.
    pub warnings: finish::Warnings,
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
            network: None,
            warnings: finish::Warnings::default(),
        }
    }
}
/// What the controller keeps past its hand-over: its Execution (and so the
/// pool's runtime), its slots and residue, publication's owner. Dropped
/// only once the finisher has returned.
///
/// With a [`Network`], the slots hold each frozen URL stage's unfinished
/// future: its request permit, its socket, its destination file and any
/// chunk write it moved onto the pool. The Execution drops first and shuts
/// its runtime down in the background, so a write still on the pool keeps
/// its own descriptor and data until it returns; the owners' drop then
/// resets each frozen connection. A library caller's peer so sees its
/// connection end after the finisher, where the process host ends it at
/// `_exit`.
#[derive(Default)]
struct Parked {
    /// The hand-over as the body builds it, here so a panic's unwind
    /// keeps what it holds (the process host's controller thread).
    handover: Option<HandOver>,
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
    if options.network.is_none() && prepared.spec.bundles().any(|b| b.url().is_some()) {
        return Err(Error::usage());
    }
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
        .run(prepared, &mut parked, None)
        .map_err(|e| terminal(e, options.lifetime.state().take()))?;
    options.observer.handed_over(&handover);
    let taken = options.lifetime.state().take();
    options.observer.taken(taken);
    let finished = finish_handover(
        &handover,
        taken,
        &Ending {
            state: options.lifetime.state(),
            fin: &fin,
            started_at,
            bound,
            progress,
            warnings: options.warnings,
            wake: None,
        },
    );
    // The library's caller writes its `exit` line from the last snapshot.
    phases::post_finish(progress, &handover, found(&finished), true);
    options.observer.finished(&finished);
    if let Some(crate::sink::Sink::Store(store)) = &handover.sink {
        store.runs.lock().clear();
        store.head_run.lock().take();
    }
    drop(handover);
    #[cfg(feature = "test-hooks")]
    if let Some(before_wakes) = options.observer.test_before_wake_drop() {
        if let Some(execution) = parked.execution.as_mut() {
            execution
                .test_retire_workers()
                .map_err(|_| Error::internal())?;
        }
        drop(parked.owners.take());
        drop(parked.owned.take());
        if let Some(runtime) = parked.execution.as_ref().and_then(Execution::runtime) {
            before_wakes(runtime);
        }
        drop(parked.residue.take());
    }
    // The owners outlive the finisher, as the process host's do until _exit.
    drop(parked);
    Ok(finished.end)
}
/// What the finisher's part reads besides the hand-over.
struct Ending<'a> {
    state: &'a State,
    fin: &'a Account,
    started_at: &'a str,
    bound: u64,
    progress: &'a Sender,
    warnings: finish::Warnings,
    wake: Option<&'a crate::sys::RegisteredWake>,
}
/// The finisher's part, after its take: the finish's progress, the 7.4
/// finish and the counts it posts, a dispatched input not committed still
/// `running` in each; the `exit` line's alone count it `not_run`. The
/// library's and the host's.
fn finish_handover(handover: &HandOver, taken: Option<Stop>, e: &Ending<'_>) -> finish::Finished {
    // The finish's snapshot at its take: the frozen view's, before its
    // reconciliation, in one line with the phase.
    e.progress.update(|s| {
        s.phase = Phase::Finish;
        phases::finish_counts(s, handover, None, false);
    });
    let started = handover.spool.as_ref().and_then(|s| s.lease.clone());
    let identity = Identity {
        started_at: e.started_at,
        memory_bound: e.bound,
        spool_peak: started.as_ref().map_or(0, |s| s.counter.peak()),
        vm_hwm: crate::progress::proc_memory("VmHWM:"),
        warnings: e.warnings,
    };
    let hard = finish::budget::Live {
        state: e.state,
        wake: e.wake,
    };
    let budget = finish::budget::Budget {
        hard: &hard,
        reconcile: matches!(&handover.sink, Some(crate::sink::Sink::Store(store)) if store.reconcile()),
    };
    let mut journal = handover.sink.as_ref().and_then(|s| match s {
        crate::sink::Sink::Store(store) => Some(crate::sink::store::journal::JournalBackend::new(
            store, budget,
        )),
        crate::sink::Sink::Directory(_) => None,
    });
    let mut no_journal = finish::budget::NoJournal;
    let backend: &mut dyn finish::budget::Backend = match journal.as_mut() {
        Some(j) => j,
        None => &mut no_journal,
    };
    let finished = finish::run(
        handover,
        taken,
        &mut finish::Finisher {
            account: e.fin,
            identity: &identity,
            // `T_hard` read live: unarmed until a deadline is armed.
            budget,
            clock: &finish::budget::BootClock,
            backend,
        },
    );
    phases::post_finish(e.progress, handover, found(&finished), false);
    finished
}
/// The `exit` line's counts (`BATCH_SPEC.md` section 10), on it alone: a
/// dispatched input not committed is `not_run` there, `running` before.
fn exit_inputs(
    mut snapshot: crate::progress::Snapshot,
    handover: &HandOver,
    finished: &finish::Finished,
) -> crate::progress::Inputs {
    phases::finish_counts(&mut snapshot, handover, found(finished), true);
    snapshot.inputs
}
/// The input whose commit the finisher's stat found at its take.
fn found(finished: &finish::Finished) -> Option<u32> {
    match finished.reconcile {
        Some(finish::Reconcile::Found(i)) => Some(i),
        _ => None,
    }
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
    /// head; every later end is a hand-over. The host gives its Execution,
    /// prepared on the controller runtime it lends; a library run makes one.
    fn run(
        &self,
        prepared: PreparedRun,
        parked: &mut Parked,
        execution: Option<Execution>,
    ) -> Result<HandOver, Error> {
        self.body(prepared, parked, execution, None, None)?;
        parked.handover.take().ok_or_else(Error::internal)
    }
    /// [`Self::run`]'s steps, building the hand-over in `parked`.
    #[allow(clippy::too_many_lines)] // 2.11's order, start to hand-over, in one place.
    fn body(
        &self,
        prepared: PreparedRun,
        parked: &mut Parked,
        execution: Option<Execution>,
        staging: Option<controller::Staging>,
        store_services: Option<store::Services>,
    ) -> Result<(), Error> {
        let network = self.options.network.as_ref();
        let local = start::execution(&prepared.spec);
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
        // A stop already observed at validation/preflight must not begin a
        // store proof. Its cancelled request would otherwise look like an
        // unreachable sink and replace the stop with a startup failure.
        if control.stopped() {
            parked.handover = Some(HandOver::new(FinishCause::Stop, Phase::Preflight));
            return Ok(());
        }
        let values = if staging.is_none() {
            network
                .map(|n| network::values(n, &prepared.spec))
                .transpose()?
        } else {
            None
        };
        let execution = match execution {
            Some(execution) => execution,
            None => Execution::prepare(&prepared).map_err(|_| Error::spawn())?,
        };
        let _ = control.wake.set(execution.wake());
        let execution = parked.execution.insert(execution);
        let staging = match (staging, network, values) {
            (Some(staging), _, _) => Some(staging),
            (_, Some(n), Some(values)) => Some(network::staging(
                n,
                values,
                execution,
                control.clone(),
                &prepared,
            )?),
            _ => None,
        };
        let sink = match &prepared.spec.sink {
            crate::spec::model::Sink::Directory { .. } => crate::sink::Sink::Directory(
                phases::preflight(&local, self.sink_fs.clone(), self.fin)?,
            ),
            crate::spec::model::Sink::ObjectStore(_) => store::preflight(
                prepared.spec.clone(),
                network,
                execution,
                self.fin,
                &*control,
                store_services,
                staging
                    .as_ref()
                    .and_then(|s| s.wakes.as_ref())
                    .map(|w| w[0].clone()),
            )?,
        };
        progress.update(|s| s.phase = Phase::Start);
        let started = match start::spool::start_recover(
            &local,
            self.spool_fs,
            &|| control.stopped(),
            &|root, canonical| {
                if let crate::sink::Sink::Store(store) = &sink
                    && let Some(runtime) = execution.runtime()
                {
                    store.recover(root, canonical, runtime, &*control);
                }
            },
        ) {
            Ok(started) => Arc::new(started),
            // A stop before or during the spool's startup or cleanup is
            // 7.4's first row, decided by the finisher like any later stop.
            Err(e) if e.class == start::spool::Class::Stopped => {
                let handover = parked
                    .handover
                    .insert(HandOver::new(FinishCause::Stop, Phase::Start));
                handover.sink = Some(sink);
                return Ok(());
            }
            Err(e) => return Err(errors::spool(&e)),
        };
        if let crate::sink::Sink::Store(store) = &sink {
            *store.journal.lock() = Some(
                Shared::new(
                    crate::spool::journal::Journal::new(&started.run).map_err(Error::from)?,
                )
                .map_err(|_| Error::memory())?,
            );
        }
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
        .retained(started.clone(), sink.clone());
        let hooks = self.options.controller_hooks.clone();
        let mut controller = match staging {
            Some(staging) => {
                Controller::networked(prepared, spool, control.clone(), hooks, staging)
            }
            None => Controller::new(prepared, spool, control.clone(), hooks),
        }
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
        let handover = parked
            .handover
            .insert(HandOver::new(FinishCause::Stop, Phase::Start));
        handover.sink = Some(sink.clone());
        handover.spool = Some(finish::Spool {
            run: started.run.clone(),
            done: done.clone(),
            fs: fs.clone(),
            lease: Some(started.clone()),
        });
        if control.stopped() {
            // 7.4's first row: the `running` write was never sent.
            return Ok(());
        }
        let acknowledged = store::running(
            controller.state(),
            &sink,
            self.fin,
            &Identity {
                started_at: self.started_at,
                memory_bound: self.bound,
                spool_peak: 0,
                vm_hwm: 0,
                warnings: self.options.warnings,
            },
            execution,
            &(control.clone() as Arc<dyn Control>),
        )?;
        handover.running =
            acknowledged || matches!(&sink, crate::sink::Sink::Store(store) if store.reconcile());
        if !handover.running {
            return Ok(());
        }
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
        handover.phase = reached;
        if boundary != Boundary::ReadyForWinners {
            stamp(handover, reached);
            return Ok(());
        }
        progress.update(|s| {
            s.phase = Phase::Winners;
            s.memory.slots_reserved = 0;
        });
        handover.phase = Phase::Winners;
        self.options.observer.seam(Seam::Selection);
        if let Some(missing) = facts
            .inputs
            .iter()
            .filter_map(|input| input.received.and_then(|attempt| attempt.missing_prefab))
            .min()
        {
            if !control.stopped() {
                handover.cause = FinishCause::UnknownPrefab(missing);
            }
            stamp(handover, Phase::Winners);
            return Ok(());
        }
        let Some((workspace, handoff)) = phases::select(
            &facts,
            &started,
            &account,
            &control,
            execution,
            &sink,
            &fs,
            #[cfg(feature = "test-hooks")]
            self.options.observer.sort_options(),
        ) else {
            stamp(handover, Phase::Winners);
            return Ok(());
        };
        let concurrency = facts.spec.download_concurrency;
        progress.update(|s| {
            s.phase = Phase::Publish;
            s.outputs.winners = handoff.counts.winners;
            s.memory.slots_reserved = 0;
        });
        let winners = handoff.counts.winners;
        // Parked before phase 7's setup, so a panic there hands over the
        // selected families; promoted to the publication owner below.
        handover.selected = Some(Selected::Parts { workspace, handoff });
        handover.phase = Phase::Publish;
        let archives = u64::try_from(facts.spec.archives.len()).map_err(|_| Error::internal())?;
        self.options.observer.seam(Seam::Outcomes);
        let Some(outcomes) =
            phases::outcomes(&started, (winners, archives), &account, (&*control, &*fs))
        else {
            stamp(handover, Phase::Publish);
            return Ok(());
        };
        let Some(Selected::Parts { workspace, handoff }) = handover.selected.take() else {
            return Err(Error::internal());
        };
        let owned = owner_slot.write(publish::Owned {
            facts,
            done,
            workspace,
            handoff,
            sink,
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
        // Here before publication, so a panic's hand-over reads its outcomes.
        handover.selected = Some(Selected::Owned(owned.clone()));
        let published = match &owned.sink {
            crate::sink::Sink::Directory(_) => publish::parallel(&owned, execution, concurrency),
            crate::sink::Sink::Store(store) => publish::store::run(&owned, store, execution),
        };
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
        stamp(handover, Phase::Publish);
        Ok(())
    }
}
/// The phase reached and the hand-over instant, sampled as it is made.
fn stamp(handover: &mut HandOver, phase: Phase) {
    handover.phase = phase;
    handover.at = std::time::Instant::now();
    handover.boot = crate::lifetime::Boot::now().ok();
}

#[cfg(test)]
mod tests;
