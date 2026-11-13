//! Bounded workers. The sole dispatch lock covers only idle receive (W3),
//! and is dropped before preparation, adapters or any core call.
use crate::{
    attempt::{self, AttemptArgs, AttemptLimits, AttemptView, StagedBundle},
    control::{Control, Culprit},
    controller::{Controller, Hooks, Point, PreparedSpool},
    job::{End, RunFailure},
    peek::{self, PeekArgs, PeekEnd},
    phase1::JobKind,
    spool::{Directory, OwnedFile},
    sys::Wake,
    view::Graph,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender},
    },
};
use unityrs_core::{RawStr, limits::Limits};
use unityrs_export::ngui;

pub(crate) struct Dispatch {
    pub kind: JobKind,
    pub id: u32,
    pub directory: Option<Arc<Directory>>,
    pub peek_file: Option<Arc<Mutex<Option<OwnedFile>>>>,
}
pub(crate) struct Context {
    spec: Arc<crate::spec::model::Spec>,
    graph: Arc<Graph>,
    extraction: Arc<attempt::Prepared>,
    words: Arc<[peek::word::Word]>,
    spool: PreparedSpool,
    control: Arc<dyn Control>,
    hooks: Arc<dyn Hooks>,
}
impl Context {
    pub fn new(c: &Controller) -> Self {
        Self {
            spec: c.prepared.spec.clone(),
            graph: c.prepared.graph.clone(),
            extraction: c.prepared.extraction.clone(),
            words: c.prepared.peek_words.clone(),
            spool: c.spool.clone(),
            control: c.control.clone(),
            hooks: c.hooks.clone(),
        }
    }
}
/// Same stack rule as the dump, without a dependency on that crate.
#[must_use]
pub fn worker_stack(limits: &Limits) -> usize {
    const LEVEL: usize = if cfg!(debug_assertions) {
        12 << 10
    } else {
        3 << 10
    };
    let levels = usize::try_from(limits.max_container_depth)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    limits
        .stack_needed()
        .saturating_mul(2)
        .saturating_add(levels.saturating_mul(LEVEL))
}
pub(crate) fn run(
    context: &Context,
    receive: &Mutex<Receiver<Dispatch>>,
    send: &SyncSender<End>,
    wake: &Wake,
) {
    loop {
        let job = {
            let Ok(receiver) = receive.lock() else {
                return;
            };
            receiver.recv()
        };
        let Ok(job) = job else {
            return;
        };
        let kind = job.kind;
        let id = job.id;
        // Adapter construction, file-owner publication and the pre-send hook
        // are covered too, not merely B4's extraction function.
        let before = crate::alloc::refusals();
        let mut produced = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let end = execute(context, &job)?;
            produced = Some(end);
            context.hooks.point(Point::BeforeSend(kind, id));
            Ok::<_, ()>(end)
        }));
        let end = match result {
            Ok(Ok(end)) if crate::alloc::refusals() == before => end,
            _ => failed(
                kind,
                id,
                if crate::alloc::refusals() == before {
                    attempt::ReasonCode::Internal
                } else {
                    attempt::ReasonCode::HostMemory
                },
            ),
        };
        let end = match (end, produced) {
            (End::Attempt(mut e), Some(End::Attempt(old))) => {
                e.slot_releasable &= old.slot_releasable;
                e.accounts = old.accounts;
                End::Attempt(e)
            }
            (End::Peek(mut e), Some(End::Peek(old))) => {
                e.slot_releasable &= old.slot_releasable;
                e.accounts = old.accounts;
                End::Peek(e)
            }
            (e, _) => e,
        };
        let failure = match end {
            End::Attempt(e) if e.state == attempt::State::RunFailure => Some(RunFailure {
                reason: e.reason.unwrap_or(attempt::ReasonCode::Internal),
                detail: e.detail,
            }),
            End::Peek(e) if e.state == peek::State::RunFailure => e.failure,
            _ => None,
        };
        if let Some(failure) = failure {
            context.control.request_stop(
                failure,
                match kind {
                    JobKind::Attempt => Culprit::Input(id),
                    JobKind::Peek => Culprit::Bundle(id),
                },
            );
        }
        // No worker owner/reference survives its end. One fixed message per
        // dispatch; capacity is threads, so no admitted worker can block here.
        drop(job);
        if send.try_send(end).is_err() {
            context.control.request_stop(
                RunFailure::new(attempt::ReasonCode::Internal),
                Culprit::None,
            );
        }
        // The observer can hold the send-to-wake seam without retaining a
        // job owner. Its panic requests a stop, never a second end.
        if catch_unwind(AssertUnwindSafe(|| {
            context.hooks.point(Point::EndQueued(kind, id));
        }))
        .is_err()
        {
            context.control.request_stop(
                RunFailure::new(attempt::ReasonCode::Internal),
                Culprit::None,
            );
        }
        if wake.notify().is_err() {
            context
                .control
                .request_stop(RunFailure::new(attempt::ReasonCode::HostIo), Culprit::None);
        }
    }
}
fn failed(kind: JobKind, id: u32, reason: attempt::ReasonCode) -> End {
    match kind {
        JobKind::Attempt => {
            let mut e = attempt::AttemptEnd::new(u64::from(id));
            e.run_failure(reason);
            End::Attempt(e)
        }
        JobKind::Peek => End::Peek(PeekEnd {
            bundle: id,
            state: peek::State::RunFailure,
            outcome: None,
            failure: Some(RunFailure::new(reason)),
            input_peak: 0,
            decode_work: 0,
            claimed_bytes: 0,
            accounts: crate::budget::Snapshot::default(),
            slot_releasable: true,
        }),
    }
}
fn execute(context: &Context, job: &Dispatch) -> Result<End, ()> {
    let hooks = Adapter {
        context,
        id: job.id,
    };
    let spec = &context.spec;
    let input = spec.bundle(job.id as usize).ok_or(())?;
    let directory = context.spool.input().map_err(|_| ())?;
    let own_path =
        crate::stage::try_path(&directory, job.id, spec.input_count(), false).map_err(|_| ())?;
    let name = staged_name(&own_path);
    match job.kind {
        JobKind::Peek => {
            let mut file = None;
            let end = peek::run_peek(PeekArgs {
                bundle: job.id,
                inputs: spec.input_count(),
                staged: &own_path,
                name: &name,
                m: spec.max_decoded_bytes,
                word: context.words.get(job.id as usize).ok_or(())?,
                file: &mut file,
                directory: &directory,
                counter: context.spool.counter.clone(),
                fs: context.spool.fs.clone(),
                stop: &*context.control,
                hooks: &hooks,
            });
            if let Some(file) = file {
                job.peek_file
                    .as_ref()
                    .ok_or(())?
                    .lock()
                    .map_err(|_| ())?
                    .replace(file);
            }
            Ok(End::Peek(end))
        }
        JobKind::Attempt => {
            let mut walk = crate::view::BoundedWalker::new(spec.max_view_bundles as usize);
            let indices = walk
                .view(&context.graph, job.id, spec.max_view_bundles)
                .map_err(|_| ())?;
            let paths: Vec<_> = indices
                .iter()
                .map(|&i| {
                    crate::stage::try_path(&directory, i, spec.input_count(), false).map_err(|_| ())
                })
                .collect::<Result<_, _>>()?;
            let names: Vec<_> = paths.iter().map(|p| staged_name(p)).collect();
            let members: Vec<_> = indices
                .iter()
                .zip(&paths)
                .zip(&names)
                .map(|((&i, path), name)| {
                    spec.bundle(i as usize)
                        .map(|b| StagedBundle {
                            id: b.id(),
                            name,
                            path,
                        })
                        .ok_or(())
                })
                .collect::<Result<_, _>>()?;
            let keys: Vec<_> = spec.archives.iter().map(|a| a.key.as_str()).collect();
            let peeks = peek::PeekBits::new(spec, &context.graph, &context.words, &directory)
                .map_err(|_| ())?;
            Ok(End::Attempt(attempt::run_attempt(AttemptArgs {
                view: AttemptView {
                    input_index: u64::from(job.id),
                    input: StagedBundle {
                        id: input.id(),
                        name: &name,
                        path: &own_path,
                    },
                    members: &members,
                },
                options: &context.extraction.options,
                prefix: match &spec.sink {
                    crate::spec::model::Sink::ObjectStore(s) => &s.prefix,
                    crate::spec::model::Sink::Directory { .. } => "",
                },
                members: &context.extraction.members,
                archive_keys: &keys,
                limits: attempt_limits(spec),
                peeks: &peeks,
                directory: job.directory.as_ref().ok_or(())?,
                stop: &*context.control,
                hooks: &hooks,
            })))
        }
    }
}
fn staged_name(path: &std::path::Path) -> RawStr {
    RawStr::from(
        path.file_name()
            .map_or(&b""[..], std::ffi::OsStr::as_encoded_bytes),
    )
}
struct Adapter<'a> {
    context: &'a Context,
    id: u32,
}
impl ngui::Hooks for Adapter<'_> {
    fn stopped(&self) -> bool {
        self.context.control.stopped() || self.context.hooks.stopped()
    }
    fn meter(&self, meter: &unityrs_core::limits::Meter) {
        self.context.hooks.meter(meter);
    }
    fn environment(&self, env: &unityrs_core::env::Environment) {
        self.context.hooks.environment(env);
    }
    fn sprite_group(&self, group: usize, file: &RawStr, path_id: i64) {
        self.context.hooks.sprite_group(group, file, path_id);
    }
    #[cfg(feature = "test-hooks")]
    fn decoder_fault(&self) {
        self.context.hooks.decoder_fault();
    }
    #[cfg(feature = "test-hooks")]
    fn environment_fault(
        &self,
        env: &unityrs_core::env::Environment,
    ) -> Option<unityrs_core::Error> {
        self.context.hooks.environment_fault(env)
    }
}
impl attempt::Hooks for Adapter<'_> {
    fn point(&self, point: attempt::Point) {
        self.context.hooks.point(Point::Attempt(self.id, point));
    }
}
impl peek::Hooks for Adapter<'_> {
    fn point(&self, point: peek::Point) {
        self.context.hooks.point(Point::Peek(self.id, point));
    }
}

fn attempt_limits(spec: &crate::spec::model::Spec) -> AttemptLimits {
    AttemptLimits {
        m: spec.max_decoded_bytes,
        d: spec.dep_cache_bytes,
        max_attempt_evictions: u64::from(spec.max_attempt_evictions),
        max_export_work: spec.max_export_work,
    }
}
