//! A library run's URL staging: the header values read at preflight, then,
//! once the Execution's runtime exists, the engine's HTTP service on a
//! socket table of the run's own and the controller's request permits.
use super::Error;
use crate::{
    control::Control,
    controller::Staging,
    execution::Execution,
    http::{self, permit::PermitPool},
    lifetime::Boot,
    phase1::PreparedRun,
    spec::model::Spec,
    stage::{
        env::{self, Environment},
        slot::Urls,
        timers::{BootClock, Context},
    },
    sys::{RegisteredWake, reset::SocketTable},
};
use std::sync::Arc;

/// What stages a library run's URL bundles. Without one, a URL is refused,
/// while the process host prepares its own transport and staging.
///
/// The run builds the engine's own service: the system resolver, compiled
/// roots, a socket table of its own and the controller's `download_concurrency`
/// permits at base 0. Its owners are parked with the controller's past a
/// stop and dropped only after the finisher returns, which resets each
/// frozen request's connection then (the host keeps them until `_exit`).
pub struct Network {
    /// Where preflight reads each `{"env": NAME}` header variable, once.
    pub environment: Arc<dyn Environment>,
    /// The deadline policy's cutoff (`lifetime::Policy`): no try or retry
    /// starts at or past it. A stage the cutoff ends is not received; the
    /// deadline's stop, which the host's lifecycle requests at the cutoff
    /// and a library caller requests itself, ends the run.
    pub cutoff: Option<Boot>,
    /// Test builds only: a resolver in place of the system's and extra
    /// trust anchors (`batch.md` 2.11's `resolve` and `extra_roots`).
    #[cfg(any(test, feature = "test-hooks"))]
    pub hooks: Option<Hooks>,
}
#[cfg(any(test, feature = "test-hooks"))]
pub struct Hooks {
    pub resolver: Arc<dyn http::resolve::Resolver>,
    pub extra_roots: Vec<rustls::pki_types::CertificateDer<'static>>,
}
impl Network {
    /// The process environment, no cutoff, no hooks.
    #[must_use]
    pub fn system() -> Self {
        Self {
            environment: Arc::new(env::System),
            cutoff: None,
            #[cfg(any(test, feature = "test-hooks"))]
            hooks: None,
        }
    }
}
/// Preflight (2.11 step 6): each header variable read once, before any
/// thread starts; a refusal is 69 with the variable as its detail.
pub(super) fn values(network: &Network, spec: &Spec) -> Result<Arc<env::Values>, Error> {
    match env::Values::resolve(spec, &*network.environment) {
        Ok(values) => Ok(Arc::new(values)),
        Err(env::Error::Refused(refusal, name)) => Err(Error::header_env(refusal, name)),
        Err(env::Error::Memory) => Err(Error::memory()),
    }
}
pub(super) fn staging(
    network: &Network,
    values: Arc<env::Values>,
    execution: &Execution,
    control: Arc<dyn Control>,
    prepared: &PreparedRun,
) -> Result<Staging, Error> {
    let spec = &prepared.spec;
    let pool = execution.handle().clone();
    // The pass's stop and end wakes, registered with the runtime here,
    // before the running head (2.11 step 4): the registration allocates.
    let wakes = RegisteredWake::new(&execution.wake(), &pool)
        .and_then(|stop| RegisteredWake::new(&execution.end_wake(), &pool).map(|end| [stop, end]))
        .map_err(|_| Error::spawn())?;
    let service = http::Service::new(pool.clone(), Arc::new(SocketTable::new()))
        .map_err(|_| Error::internal())?;
    #[cfg(any(test, feature = "test-hooks"))]
    let service = match &network.hooks {
        Some(hooks) => service
            .with_test_hooks(hooks.resolver.clone(), &hooks.extra_roots)
            .map_err(|_| Error::internal())?,
        None => service,
    };
    let permits = usize::try_from(spec.download_concurrency)
        .ok()
        .and_then(|n| PermitPool::with_base(0, n).ok())
        .ok_or_else(Error::internal)?;
    let context = Arc::new(Context {
        clock: Arc::new(BootClock::default()),
        control,
        cutoff: BootClock::cutoff(network.cutoff),
    });
    let [stop, end] = wakes;
    let wakes = [
        crate::sys::Shared::new(stop).map_err(|_| Error::memory())?,
        crate::sys::Shared::new(end).map_err(|_| Error::memory())?,
    ];
    Ok(Staging {
        launch: Arc::new(Urls {
            parsed: prepared.urls.clone(),
            names: prepared.header_names.clone(),
            transport: Arc::new(service),
            pool,
            context,
            values,
        }),
        permits,
        wakes: Some(wakes),
    })
}
