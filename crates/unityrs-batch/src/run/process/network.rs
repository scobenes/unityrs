//! Process-lived transports, prepared in step 4 on the host's two
//! runtimes and its one reset table. No runtime or table is made here.
use super::{Error, Host};
use crate::{
    controller::Staging,
    execution::Execution,
    http::{
        self,
        permit::{FINISHER_BASE, PermitPool},
    },
    lifetime::{Policy, Process, Runtimes},
    stage::{
        env,
        slot::Urls,
        timers::{BootClock, Context},
    },
    sys::RegisteredWake,
};
use std::sync::Arc;

fn transport_setup(error: http::Error) -> Error {
    if error == http::Error::Memory {
        Error::memory()
    } else {
        Error::startup("http", None)
    }
}

/// Startup storage retained to process exit even though the directory
/// finisher currently needs no HTTP requests. Services/configuration belong
/// to fixed `C_base`; both request tables follow the `m_pool` slope.
pub(super) struct Resources {
    controller: Arc<http::Service>,
    _finisher: http::Service,
    _permits: PermitPool,
}
impl Resources {
    pub(super) fn prepare(
        runtimes: &Runtimes,
        process: &Process,
        dc: u32,
        #[cfg(feature = "test-hooks")] hooks: &mut super::TestHooks,
        #[cfg(feature = "test-hooks")] log: Option<Arc<super::Log>>,
    ) -> Result<(Self, PermitPool), Error> {
        let controller = http::Service::new(
            runtimes.controller.handle().clone(),
            process.sockets.clone(),
        )
        .map_err(transport_setup)?;
        #[cfg(feature = "test-hooks")]
        let controller = {
            let resolver = Arc::new(hooks.resolve.take().unwrap_or_default().with_log(log));
            controller
                .with_test_hooks(resolver, &hooks.extra_roots)
                .map_err(transport_setup)?
        };
        let finisher = controller.on_runtime(runtimes.finisher.handle().clone());
        let capacity = usize::try_from(dc).map_err(|_| Error::internal())?;
        let controller_permits = PermitPool::with_base(0, capacity).map_err(|_| Error::memory())?;
        let finisher_permits =
            PermitPool::with_base(FINISHER_BASE, capacity).map_err(|_| Error::memory())?;
        Ok((
            Self {
                controller: Arc::new(controller),
                _finisher: finisher,
                _permits: finisher_permits,
            },
            controller_permits,
        ))
    }
}

impl Host {
    /// Step 6, before host checks and spawns. The immutable values and spec
    /// live in the process host so the masker borrows them without copies.
    pub(super) fn headers(&'static self) -> Result<(), Error> {
        let mut values = env::Values::default();
        let resolved = values
            .resolve_into(&self.spec, &env::System)
            .map_err(|e| match e {
                env::Error::Memory => Error::memory(),
                env::Error::Refused(reason, name) => Error::header_env(reason, name),
            });
        let values = self.values.get_or_init(|| Arc::new(values));
        let mut secrets = self
            .mask_buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(Error::internal)?;
        values
            .secrets(&self.spec, &mut secrets)
            .map_err(|_| Error::memory())?;
        self.masked.set(secrets).map_err(|_| Error::internal())?;
        resolved
    }
    /// Step 7, after workers exist, before the controller starts and before
    /// the running head. Both wake registrations allocate only here.
    pub(super) fn staging(
        &'static self,
        execution: &Execution,
        prepared: &crate::phase1::PreparedRun,
        policy: Policy,
        permits: PermitPool,
    ) -> Result<Staging, Error> {
        let pool = execution.handle().clone();
        let wakes = RegisteredWake::new(&execution.wake(), &pool)
            .and_then(|stop| {
                RegisteredWake::new(&execution.end_wake(), &pool).map(|end| [stop, end])
            })
            .map_err(|_| Error::startup("epoll_ctl", None))?;
        let context = Arc::new(Context {
            clock: Arc::new(BootClock::default()),
            control: Arc::new(super::super::Lifetime::process(&self.process)),
            cutoff: BootClock::cutoff(policy.cutoff),
        });
        Ok(Staging {
            launch: Arc::new(Urls {
                parsed: prepared.urls.clone(),
                names: prepared.header_names.clone(),
                transport: self
                    .network
                    .get()
                    .ok_or_else(Error::internal)?
                    .controller
                    .clone(),
                pool,
                context,
                values: self.values.get().ok_or_else(Error::internal)?.clone(),
            }),
            permits,
            wakes: Some(wakes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_refused_transport_startup_allocation_keeps_host_memory() {
        let error = transport_setup(http::Error::Memory);
        assert_eq!(error.code, crate::reason::Code::Host);
        assert_eq!(error.reason, "host_memory");
        assert_eq!(
            transport_setup(http::Error::InvalidRequest).reason,
            "startup_failed"
        );
    }
}
