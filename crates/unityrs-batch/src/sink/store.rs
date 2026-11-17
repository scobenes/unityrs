//! Run-owned store clients and fences. Controller and finisher connections
//! use distinct runtime services and socket-table ranges (batch 2.9, 2.11).
pub(crate) mod errors;
pub(crate) mod finish;
pub(crate) mod journal;
use crate::{
    control::Control,
    http::{
        self,
        permit::{FINISHER_BASE, PermitPool},
    },
    phase::{Account, Buffer, Error},
    spec::model::{ObjectStore, Sink, Spec},
    store::{
        Kind,
        attempt::{Policy, Tries, TrySlot},
        client,
        connector::Connector,
        head,
        put::Request,
    },
    sys::{Lock, Shared},
};
use object_store::path::Path;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

pub(crate) enum Runtime {
    Own(Option<tokio::runtime::Runtime>),
    Lent(&'static tokio::runtime::Runtime),
}
impl Runtime {
    pub(crate) fn get(&self) -> Option<&tokio::runtime::Runtime> {
        match self {
            Self::Own(r) => r.as_ref(),
            Self::Lent(r) => Some(r),
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Self::Own(runtime) = self
            && let Some(runtime) = runtime.take()
        {
            runtime.shutdown_background();
        }
    }
}
/// Fixed figures sampled through the hand-over; row requests may still raise
/// unsettled after publication, so the final head samples that atomic last.
#[derive(Clone, Copy, Debug, Default)]
pub struct Figures {
    pub verified: Option<bool>,
    pub verified_by_size: u64,
    pub resume_list_failed: u64,
    pub verify_list_failed: u64,
    pub etag_not_md5: u64,
    pub abort_failed: u64,
}
pub(crate) type HeadRun =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), super::Error>> + Send>>;
pub struct Store {
    pub(crate) spec: Arc<Spec>,
    pub(crate) clients: [client::Store; 3],
    finisher: client::Store,
    runtime: Runtime,
    pub(crate) slots: Vec<Arc<TrySlot>>,
    pub(crate) accounts: Vec<(Account, crate::store::multipart::PartBudget)>,
    pub(crate) runs: Lock<Vec<Option<crate::publish::store::Run>>>,
    fin_slots: Vec<Arc<TrySlot>>,
    pub(crate) policy: Policy,
    pub(crate) unsettled: AtomicU64,
    fence: head::Fence,
    acknowledged: Lock<Option<crate::store::etag::Etag>>,
    pub(crate) running_account: Account,
    pub(crate) head_run: Lock<Option<HeadRun>>,
    head: Path,
    pub(crate) journal: Lock<Option<Shared<crate::spool::journal::Journal>>>,
    pub(crate) figures: Lock<Figures>,
    pub(crate) wake: Shared<crate::sys::RegisteredWake>,
    pub(crate) listed: Lock<[Option<crate::publish::store::Listed>; 256]>,
    pub(crate) tags: Lock<Option<crate::spool::random::Mutable>>,
}
impl Store {
    #[allow(clippy::too_many_arguments)] // Prepared before the running head, on the host's existing services.
    #[allow(clippy::large_stack_arrays)] // Fixed at the spec's 256 archives, retained in C_base before running.
    #[allow(clippy::too_many_lines)] // One pre-head owner constructs and retains both services' permits.
    pub(crate) fn prepare(
        spec: Arc<Spec>,
        controller: http::Service,
        finisher: http::Service,
        runtime: Runtime,
        credentials: &object_store::aws::AwsCredentialProvider,
        wake: Shared<crate::sys::RegisteredWake>,
        cutoff: Option<tokio::time::Instant>,
    ) -> Result<Self, Error> {
        let Sink::ObjectStore(settings) = &spec.sink else {
            return Err(Error::internal());
        };
        let endpoint = settings
            .endpoint
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|_| Error::internal())?;
        let address = client::Settings {
            bucket: &settings.bucket,
            region: &settings.region,
            endpoint: endpoint.as_ref(),
            path_style: settings.path_style,
        };
        let connector = Connector::new(
            controller,
            client::origin(&address).map_err(errors::fault)?,
            Duration::from_millis(u64::from(settings.timeout_ms)),
        );
        let clients = [
            client::build(&address, Some("image/png"), credentials, &connector),
            client::build(&address, Some("application/json"), credentials, &connector),
            client::build(&address, Some("application/zip"), credentials, &connector),
        ];
        let [png, json, zip] = clients;
        let finisher = client::build(
            &address,
            None,
            credentials,
            &Connector::new(
                finisher,
                client::origin(&address).map_err(errors::fault)?,
                Duration::from_millis(u64::from(settings.timeout_ms)),
            ),
        )
        .map_err(errors::fault)?;
        let pool = PermitPool::new(spec.download_concurrency as usize)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(spec.download_concurrency as usize)
            .map_err(|_| Error::memory())?;
        let mut accounts = Vec::new();
        accounts
            .try_reserve_exact(spec.download_concurrency as usize)
            .map_err(|_| Error::memory())?;
        let mut runs = Vec::new();
        runs.try_reserve_exact(spec.download_concurrency as usize)
            .map_err(|_| Error::memory())?;
        for _ in 0..spec.download_concurrency {
            slots.push(TrySlot::new(
                pool.acquire().ok_or_else(Error::internal)?,
                Kind::Put,
            ));
            accounts.push((
                Account::new(spec.max_inflight_uncompressed_bytes),
                crate::store::multipart::PartBudget::new(),
            ));
            runs.push(None);
        }
        let fin_pool =
            PermitPool::with_base(FINISHER_BASE, spec.download_concurrency as usize + 1)?;
        let mut fin_slots = Vec::new();
        fin_slots
            .try_reserve_exact(spec.download_concurrency as usize + 1)
            .map_err(|_| Error::memory())?;
        for _ in 0..=spec.download_concurrency {
            fin_slots.push(TrySlot::new(
                fin_pool.acquire().ok_or_else(Error::internal)?,
                Kind::Put,
            ));
        }
        let policy = Policy {
            attempts: settings.attempts,
            timeout: Duration::from_millis(u64::from(settings.timeout_ms)),
            backoff_initial_ms: spec.http.backoff_initial_ms,
            backoff_max_ms: spec.http.backoff_max_ms,
            cutoff,
        };
        let head = Self::key_for(&settings.prefix, &spec.run_id, "/manifest.json")?;
        Ok(Self {
            spec,
            clients: [
                png.map_err(errors::fault)?,
                json.map_err(errors::fault)?,
                zip.map_err(errors::fault)?,
            ],
            finisher,
            runtime,
            slots,
            accounts,
            runs: Lock::new(runs),
            fin_slots,
            policy,
            unsettled: AtomicU64::new(0),
            fence: head::Fence::default(),
            acknowledged: Lock::new(None),
            running_account: Account::new(crate::bound::C_FIN),
            head_run: Lock::new(None),
            head,
            journal: Lock::new(None),
            figures: Lock::new(Figures::default()),
            wake,
            listed: Lock::new([None; 256]),
            tags: Lock::new(None),
        })
    }
    fn key_for(prefix: &str, run: &str, suffix: &str) -> Result<Path, Error> {
        let mut key = String::new();
        key.try_reserve_exact(prefix.len() + 9 + run.len() + suffix.len())
            .map_err(|_| Error::memory())?;
        key.push_str(prefix);
        key.push_str("_unityrs/");
        key.push_str(run);
        key.push_str(suffix);
        Path::parse(key).map_err(|_| Error::internal())
    }
    pub(crate) fn settings(&self) -> &ObjectStore {
        match &self.spec.sink {
            Sink::ObjectStore(s) => s,
            Sink::Directory { .. } => unreachable_settings(),
        }
    }
    pub(crate) fn fin_slot(&self) -> Result<&Arc<TrySlot>, crate::store::put::Error> {
        available(&self.fin_slots)
    }
    pub(crate) fn slot(&self, index: usize) -> Result<&Arc<TrySlot>, Error> {
        self.slots.get(index).ok_or_else(Error::internal)
    }
    pub(crate) fn account(&self, index: usize) -> Result<&Account, Error> {
        self.accounts
            .get(index)
            .map(|a| &a.0)
            .ok_or_else(Error::internal)
    }
    pub(crate) fn parts(
        &self,
        index: usize,
    ) -> Result<&crate::store::multipart::PartBudget, Error> {
        self.accounts
            .get(index)
            .map(|a| &a.1)
            .ok_or_else(Error::internal)
    }
    pub(crate) fn tries<'a>(&'a self, control: &'a dyn Control) -> Tries<'a> {
        Tries {
            policy: self.policy,
            control,
            unsettled: &self.unsettled,
        }
    }
    pub(crate) async fn preflight(
        &self,
        account: &Account,
        control: &dyn Control,
    ) -> Result<(), super::Error> {
        let mut buffer = Buffer::new(
            account.meter(),
            usize::try_from(crate::store::HEAD_CAP)
                .map_err(|_| errors::phase(Error::internal()))?,
        )
        .map_err(errors::phase)?;
        buffer.fill(0);
        let tries = self.tries(control);
        let request = Request::new(
            &self.clients[1],
            &tries,
            self.slot(0).map_err(errors::phase)?,
            self.deadline(),
        );
        let head = head::preflight(
            &request,
            &self.settings().prefix,
            &self.head,
            &self.spec.run_id,
            self.spec.attempt,
            &mut buffer,
        )
        .await
        .map_err(errors::head)?;
        self.fence.initialize(head.map(|h| h.etag));
        Ok(())
    }
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        self.policy
            .cutoff
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_hours(8760))
    }
    pub(crate) async fn running(
        &self,
        bytes: &[u8],
        account: &Account,
        control: &dyn Control,
    ) -> Result<(), super::Error> {
        let tries = self.tries(control);
        let request = Request::new(
            &self.clients[1],
            &tries,
            self.slot(0).map_err(errors::phase)?,
            self.deadline(),
        );
        self.write(&request, bytes, true, account).await
    }
    async fn write(
        &self,
        request: &Request<'_>,
        bytes: &[u8],
        running: bool,
        account: &Account,
    ) -> Result<(), super::Error> {
        let payload = crate::store::put::Payload::read(account, bytes.len(), &mut &bytes[..])
            .map_err(|e| errors::head(head::Error::Operation(e)))?;
        let mut buffer = Buffer::new(
            account.meter(),
            usize::try_from(crate::store::HEAD_CAP)
                .map_err(|_| errors::phase(Error::internal()))?,
        )
        .map_err(errors::phase)?;
        buffer.fill(0);
        let final_fence = head::Fence::new(self.acknowledged.lock().or(self.fence.last()));
        let fence = if running { &self.fence } else { &final_fence };
        let result = head::Write {
            key: &self.head,
            run_id: &self.spec.run_id,
            identity: crate::manifest::head::Identity {
                attempt: self.spec.attempt,
                write_seq: u64::from(!running),
            },
            bytes: payload.into_bytes(),
            conditional: self.spec.manifest_fencing == crate::spec::model::Fencing::Conditional,
        }
        .publish(request, fence, &mut buffer)
        .await;
        match result {
            Ok(etag) => {
                if running {
                    *self.acknowledged.lock() = Some(etag);
                }
                Ok(())
            }
            Err(error) => Err(errors::head(error)),
        }
    }
    pub(crate) fn reconcile(&self) -> bool {
        self.fence.history().needs_final_after_stop()
    }
    #[must_use]
    pub fn figures(&self) -> Figures {
        *self.figures.lock()
    }
    #[must_use]
    pub fn unsettled(&self) -> u64 {
        self.unsettled.load(Ordering::Acquire)
    }
    /// Test-only custody audit: retain meters without retaining the runtime
    /// or any transfer future, then check their charges after run teardown.
    #[cfg(feature = "test-hooks")]
    pub fn visit_meters(&self, mut visit: impl FnMut(&Arc<unityrs_core::limits::Meter>)) {
        visit(self.running_account.meter());
        for (account, parts) in &self.accounts {
            visit(account.meter());
            parts.visit_meter(&mut visit);
        }
    }
    #[cfg(feature = "test-hooks")]
    pub fn visit_permits(&self, mut visit: impl FnMut(crate::http::permit::ChargeProbe)) {
        for slot in self.slots.iter().chain(&self.fin_slots) {
            visit(slot.charge_probe());
        }
    }
    /// Test-only reactor custody: a sweep can keep an idle registration
    /// alive until counted teardown, independently of the last socket wake.
    #[cfg(feature = "test-hooks")]
    #[must_use]
    pub fn test_finisher_handle(&self) -> Option<&tokio::runtime::Handle> {
        self.runtime.get().map(tokio::runtime::Runtime::handle)
    }
}
// The owner is constructed only from an ObjectStore spec. Returning a fixed
// empty value keeps this checked accessor total even if that invariant breaks.
fn unreachable_settings() -> &'static ObjectStore {
    static EMPTY: ObjectStore = ObjectStore {
        bucket: String::new(),
        region: String::new(),
        prefix: String::new(),
        endpoint: None,
        path_style: false,
        timeout_ms: 1000,
        attempts: 1,
    };
    &EMPTY
}
fn available(slots: &[Arc<TrySlot>]) -> Result<&Arc<TrySlot>, crate::store::put::Error> {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    for slot in slots {
        match slot.poll_lookup(&mut cx) {
            std::task::Poll::Ready(Ok(())) => return Ok(slot),
            std::task::Poll::Ready(Err(f)) => return Err(f.into()),
            std::task::Poll::Pending => {}
        }
    }
    Err(crate::store::put::Error::Stopped)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_quarantined_finisher_lookup_does_not_strand_the_head() {
        let pool = PermitPool::with_base(FINISHER_BASE, 2).unwrap();
        let mut permit = pool.acquire().unwrap();
        let token = permit
            .begin(crate::http::permit::Operation::Lookup)
            .unwrap();
        let first = TrySlot::new(permit, Kind::Put);
        let second = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
        let slots = [first.clone(), second.clone()];
        assert!(Arc::ptr_eq(available(&slots).unwrap(), &second));
        drop(token);
        assert!(Arc::ptr_eq(available(&slots).unwrap(), &first));
    }
}
