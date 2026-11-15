//! Store preparation before the running head; all clients use the engine's
//! service. A process host lends its two services and finisher runtime.
use super::{Error, Network, errors};
use crate::{
    execution::Execution,
    phase::Account,
    sink::{
        Sink,
        store::{Runtime, Store},
    },
    sys::Shared,
};
use std::sync::Arc;

pub(super) struct Services {
    pub controller: crate::http::Service,
    pub finisher: crate::http::Service,
    pub runtime: &'static tokio::runtime::Runtime,
    pub credentials: Option<object_store::aws::AwsCredentialProvider>,
    pub cutoff: Option<crate::lifetime::Boot>,
}
pub(super) fn preflight(
    spec: Arc<crate::spec::model::Spec>,
    network: Option<&Network>,
    execution: &Execution,
    account: &Account,
    control: &dyn crate::control::Control,
    services: Option<Services>,
    wake: Option<Shared<crate::sys::RegisteredWake>>,
) -> Result<Sink, Error> {
    let cutoff = services
        .as_ref()
        .and_then(|s| s.cutoff)
        .or_else(|| network.and_then(|n| n.cutoff));
    let credentials = match services.as_ref().and_then(|s| s.credentials.clone()) {
        Some(credentials) => credentials,
        None => match network {
            Some(n) => crate::store::credentials::read(
                |name| n.environment.var(name),
                spec.deadline.as_ref().map(|d| &d.at),
            ),
            None => crate::store::credentials::Credentials::preflight(
                spec.deadline.as_ref().map(|d| &d.at),
            ),
        }
        .map_err(|reason| Error::could_not_start(reason.name()))?
        .into_provider(),
    };
    let (controller, finisher, runtime) = if let Some(s) = services {
        (s.controller, s.finisher, Runtime::Lent(s.runtime))
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(spec.download_concurrency as usize + 1)
            .thread_keep_alive(std::time::Duration::MAX)
            .build()
            .map_err(|_| Error::spawn())?;
        crate::execution::prewarm(runtime.handle(), spec.download_concurrency as usize + 1)
            .map_err(|_| Error::spawn())?;
        let controller = crate::http::Service::new(
            execution.handle().clone(),
            Arc::new(crate::sys::reset::SocketTable::new()),
        )
        .map_err(|_| Error::memory())?;
        #[cfg(any(test, feature = "test-hooks"))]
        let controller = match network.and_then(|n| n.hooks.as_ref()) {
            Some(hooks) => controller
                .with_test_hooks(hooks.resolver.clone(), &hooks.extra_roots)
                .map_err(|_| Error::memory())?,
            None => controller,
        };
        let finisher = controller.on_runtime(runtime.handle().clone());
        (controller, finisher, Runtime::Own(Some(runtime)))
    };
    let wake = match wake {
        Some(w) => w,
        None => Shared::new(
            crate::sys::RegisteredWake::new(&execution.wake(), execution.handle())
                .map_err(|_| Error::memory())?,
        )
        .map_err(|_| Error::memory())?,
    };
    let store = Shared::new(
        Store::prepare(
            spec,
            controller,
            finisher,
            runtime,
            &credentials,
            wake,
            cutoff.map(|d| {
                tokio::time::Instant::now()
                    + std::time::Duration::from_millis(
                        d.0.saturating_sub(crate::lifetime::Boot::now().map_or(u64::MAX, |n| n.0)),
                    )
            }),
        )
        .map_err(errors::running)?,
    )
    .map_err(|_| Error::memory())?;
    execution
        .runtime()
        .ok_or_else(Error::internal)?
        .block_on(store.preflight(account, control))
        .map_err(errors::sink)?;
    Ok(Sink::Store(store))
}
pub(super) fn running(
    facts: &dyn crate::finish::Header,
    sink: &Sink,
    account: &Account,
    identity: &crate::finish::Identity<'_>,
    execution: &Execution,
    control: &Arc<dyn crate::control::Control>,
) -> Result<bool, Error> {
    match sink {
        Sink::Directory(d) => crate::finish::running(facts, d, account, identity)
            .map(|()| true)
            .map_err(errors::running),
        Sink::Store(store) => {
            let mut json =
                crate::finish::running_bytes(facts, account, identity).map_err(errors::running)?;
            let encoded = json.end().map_err(errors::running)?;
            let mut bytes = crate::phase::Buffer::new(store.running_account.meter(), encoded.len())
                .map_err(errors::running)?;
            bytes.extend(encoded).map_err(errors::running)?;
            drop(json);
            let storage = store
                .running_account
                .meter()
                .charge(unityrs_core::limits::heap_cost(16 * 1024))
                .map_err(crate::phase::Error::from)
                .map_err(errors::running)?;
            let state = store.clone();
            let stop = control.clone();
            let future = async move {
                let _storage = storage;
                state.running(&bytes, &state.running_account, &*stop).await
            };
            if size_of_val(&future) > 16 * 1024 {
                return Err(Error::internal());
            }
            let boxed: Box<
                dyn std::future::Future<Output = Result<(), crate::sink::Error>> + Send,
            > = crate::alloc::try_box(future).map_err(|_| Error::memory())?;
            *store.head_run.lock() = Some(Box::into_pin(boxed));
            execution
                .runtime()
                .ok_or_else(Error::internal)?
                .block_on(std::future::poll_fn(|cx| {
                    while let std::task::Poll::Ready(result) = store.wake.poll_wait(cx) {
                        if result.is_err() {
                            return std::task::Poll::Ready(Err(Error::host_io()));
                        }
                    }
                    if control.stopped() {
                        return std::task::Poll::Ready(Ok(false));
                    }
                    let mut slot = store.head_run.lock();
                    let Some(future) = slot.as_mut() else {
                        return std::task::Poll::Ready(Err(Error::internal()));
                    };
                    match future.as_mut().poll(cx) {
                        std::task::Poll::Pending => std::task::Poll::Pending,
                        std::task::Poll::Ready(result) => {
                            *slot = None;
                            std::task::Poll::Ready(result.map(|()| true).map_err(errors::sink))
                        }
                    }
                }))
        }
    }
}
