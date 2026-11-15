//! A URL stage's owning future. One async block owns the stage, its request
//! permit and every argument its run borrows, so the controller can retain
//! it in a stage slot, polled on its runtime, and park it unfinished at a
//! hand-over: dropping it is not a stop (`url.rs`), and nothing awaits it
//! after one (a frozen network wait never returns).
use super::{
    Failure, StageEnd, env, headers,
    timers::{BootClock, Context},
    url::Stage,
};
use crate::{
    attempt::ReasonCode, http::permit::RequestPermit, job::RunFailure, spec::model::Spec,
    spool::OwnedFile,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::runtime::Handle;

/// What a finished run gives the controller: its end, and the destination's
/// owner (absent only when the run failed before it could own one).
pub struct Ended {
    pub end: StageEnd,
    pub file: Option<OwnedFile>,
    /// A stopping end's stage and permit, its response (and so its socket)
    /// still inside: the controller parks it with its owners, so a live
    /// connection is reset after the finisher, as a frozen one is.
    pub residual: Option<Residual>,
}
/// See [`Ended::residual`]. Opaque: only its drop matters.
pub struct Residual {
    _stage: Stage<crate::http::Response<crate::http::Stream>>,
    _permit: RequestPermit,
}
pub type Run = Pin<Box<dyn Future<Output = Ended> + Send>>;
/// The most an owning future may occupy. Its one allocation per launch is
/// counted inside the stage's own `STAGE_ENVELOPE` (128 KiB) charge,
/// whose pending-task bookkeeping it is; a larger future is refused as
/// `internal` rather than charged past that envelope.
pub const RUN_STORAGE: usize = 16 * 1024;
static RUN_SIZE: AtomicUsize = AtomicUsize::new(0);
/// The size of the last owning future launched (for the ledger's tests).
#[must_use]
pub fn run_size() -> usize {
    RUN_SIZE.load(Ordering::Relaxed)
}
/// Starts URL stages for the controller. Object-safe, so one controller type
/// serves the engine's transport and a test's.
pub(crate) trait Launch: Send + Sync {
    /// `file` is the bundle's fresh destination; `permit` is held by the
    /// future until its end, its socket and response dropped before it.
    fn launch(
        &self,
        spec: &Arc<Spec>,
        bundle: u32,
        file: OwnedFile,
        permit: RequestPermit,
    ) -> Result<Run, Failure>;
}
/// The engine's URL stages: its HTTP service, the boot clock and the
/// preflight's header values. Concrete, so the run future is `Send`.
pub(crate) struct Urls {
    /// Phase 1's parsed URLs (`PreparedRun::urls`), by bundle index.
    pub parsed: Arc<[(u32, url::Url)]>,
    pub transport: Arc<crate::http::Service>,
    /// The controller runtime's blocking pool: lookups and chunk writes.
    pub pool: Handle,
    pub context: Arc<Context<BootClock>>,
    pub values: Arc<env::Values>,
    /// Phase 1's parsed header names (`PreparedRun::header_names`).
    pub names: Arc<headers::Names>,
}
impl Launch for Urls {
    fn launch(
        &self,
        spec: &Arc<Spec>,
        bundle: u32,
        file: OwnedFile,
        permit: RequestPermit,
    ) -> Result<Run, Failure> {
        let stage = Stage::new(file, &permit)?;
        let (urls, spec, transport, pool, cx, values, names) = (
            self.parsed.clone(),
            spec.clone(),
            self.transport.clone(),
            self.pool.clone(),
            self.context.clone(),
            self.values.clone(),
            self.names.clone(),
        );
        let future = async move {
            let (mut stage, mut permit) = (stage, permit);
            let Some(source) = spec.bundle(bundle as usize) else {
                return Ended {
                    end: ended(bundle, internal()),
                    file: stage.file.take(),
                    residual: None,
                };
            };
            let url = urls
                .binary_search_by_key(&bundle, |(index, _)| *index)
                .ok()
                .and_then(|i| urls.get(i))
                .map(|(_, url)| url);
            let end = stage
                .run_parsed(
                    bundle,
                    source,
                    url,
                    spec.max_decoded_bytes,
                    &spec.http,
                    &headers::Prepared {
                        values: &*values,
                        names: &names,
                    },
                    &*transport,
                    &mut permit,
                    &pool,
                    &cx,
                )
                .await;
            let file = stage.file.take();
            if matches!(end.failure, Some(Failure::Run(_) | Failure::Stopped)) {
                // A stopping end: the stage requested the run's stop, or a
                // stop (or the cutoff) ended it, maybe with its response
                // still open. Nothing is closed before the finisher.
                return Ended {
                    end,
                    file,
                    residual: Some(Residual {
                        _stage: stage,
                        _permit: permit,
                    }),
                };
            }
            // The response, and so its socket, before the permit: a later
            // stage reuses this permit's socket-table entry.
            drop(stage);
            drop(permit);
            Ended {
                end,
                file,
                residual: None,
            }
        };
        let size = size_of_val(&future);
        RUN_SIZE.store(size, Ordering::Relaxed);
        if size > RUN_STORAGE {
            return Err(internal());
        }
        let boxed: Box<dyn Future<Output = Ended> + Send> = crate::alloc::try_box(future)
            .map_err(|_| Failure::Run(RunFailure::new(ReasonCode::HostMemory)))?;
        Ok(Box::into_pin(boxed))
    }
}
pub(crate) fn ended(bundle: u32, failure: Failure) -> StageEnd {
    StageEnd {
        bundle,
        failure: Some(failure),
        integrity: None,
        facts: None,
        claimed_bytes: 0,
    }
}
fn internal() -> Failure {
    Failure::Run(RunFailure::new(ReasonCode::Internal))
}
