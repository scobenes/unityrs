//! X12's library-only observation seam. No history, allocation or progress
//! sampling: the test's coherent ledger receives each granted charge and
//! return, including returns on another thread. Install before preparation
//! in a fresh child. Native-only charges need separate component evidence;
//! these events must not be used to fit a Rust-only base coefficient.
use crate::progress::Phase;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub enum Point {
    Phase(Phase),
    /// Before the normal process exit, with frozen owners still retained.
    Exit,
    /// A real phase-6 merge group, including later generations.
    SortMerge,
}

/// Callbacks must not allocate, panic or reenter account operations. Point
/// callbacks may acknowledge a deterministic hold outside allocator events.
#[derive(Clone, Copy, Debug)]
pub struct Observer {
    pub charge: fn(u64, bool),
    /// Network reservations are reported separately. They must not also be
    /// deducted from a residual whose allowance already includes `C_net`.
    pub network: fn(u64, bool),
    pub point: fn(Point),
}
static OBSERVER: OnceLock<Observer> = OnceLock::new();
static PHASES: AtomicU64 = AtomicU64::new(0);

/// One observer per child, fixed before any account exists. It cannot be
/// replaced while outstanding permits retain the original observation.
pub fn install(observer: Observer) -> Result<(), Observer> {
    OBSERVER.set(observer)
}
pub(crate) fn charge(bytes: u64, held: bool) {
    if let Some(observer) = OBSERVER.get() {
        (observer.charge)(bytes, held);
    }
}
pub(crate) fn network(bytes: u64, held: bool) {
    if let Some(observer) = OBSERVER.get() {
        (observer.network)(bytes, held);
    }
}
pub fn at(point: Point) {
    if let Some(observer) = OBSERVER.get() {
        if let Point::Phase(phase) = point {
            let bit = 1 << phase as u32;
            if PHASES.fetch_or(bit, Ordering::AcqRel) & bit != 0 {
                return;
            }
        }
        (observer.point)(point);
    }
}

/// Isolated representation probe of the real dispatch channel. Referenced
/// spool owners have their own charged lifetime; messages only share them.
/// This library-only test adapter never runs in the production engine.
pub fn probe_dispatch(cap: usize, observe: impl FnOnce()) -> Option<usize> {
    if !(1..=1024).contains(&cap) {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(cap);
    for id in 0..cap {
        tx.try_send(crate::worker::Dispatch {
            kind: crate::phase1::JobKind::Attempt,
            id: u32::try_from(id).ok()?,
            directory: None,
            peek_file: None,
        })
        .ok()?;
    }
    observe();
    drop((tx, rx));
    Some(size_of::<crate::worker::Dispatch>())
}

/// Drive the real lookup/task-cell path without opening a connection.
/// The caller owns the runtime, permit and deterministic resolver.
pub async fn probe_lookup(
    handle: &tokio::runtime::Handle,
    permit: &mut crate::http::permit::RequestPermit,
    resolver: std::sync::Arc<dyn crate::http::resolve::Resolver>,
    host: &str,
) -> Result<crate::http::resolve::Addresses, crate::http::Error> {
    crate::http::resolve::lookup(
        handle,
        permit,
        &crate::http::resolve::Source::Hook(resolver),
        host,
        80,
        None,
    )
    .await
}

/// Construct actual bounded store owners without starting pool threads or
/// issuing a request. Includes fixed setup conservatively in the probe.
pub fn probe_store_preparation(
    spec: std::sync::Arc<crate::spec::model::Spec>,
    credentials: &object_store::aws::AwsCredentialProvider,
    observe: impl FnOnce(),
) -> Option<()> {
    let runtimes = crate::lifetime::Runtimes::prepare(spec.download_concurrency).ok()?;
    let controller = crate::http::Service::new(
        runtimes.controller.handle().clone(),
        std::sync::Arc::new(crate::sys::reset::SocketTable::new()),
    )
    .ok()?;
    let finisher = controller.on_runtime(runtimes.finisher.handle().clone());
    let wake = std::sync::Arc::new(crate::sys::Wake::new().ok()?);
    let registered = crate::sys::Shared::new(
        crate::sys::RegisteredWake::new(&wake, runtimes.controller.handle()).ok()?,
    )
    .ok()?;
    let store = crate::sink::store::Store::prepare(
        spec,
        controller,
        finisher,
        crate::sink::store::Runtime::Own(Some(runtimes.finisher)),
        credentials,
        registered,
        None,
    )
    .ok()?;
    observe();
    drop(store);
    Some(())
}
