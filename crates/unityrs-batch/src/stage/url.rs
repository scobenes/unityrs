//! Standalone URL staging, driven on the caller's current-thread runtime.
//! Retain both this owner and its run future outside the controller's unwind
//! scope. B10c freezes polling at handover; dropping a run future is not stop.
use super::{
    DataFailure, Facts, Failure, Integrity, StageEnd,
    headers::Values,
    timers::{BodyTime, Clock, Context},
};
use crate::{
    attempt::ReasonCode,
    control::Culprit,
    http::{
        self, Data, Detail, Error, Payload,
        permit::{Charge, Operation, OperationToken, RequestPermit},
    },
    job::RunFailure,
    spec::model::{BundleRef, Http, Redirects},
    spool::OwnedFile,
};
use hyper::{
    Request,
    http::{
        HeaderMap,
        header::{CONTENT_LENGTH, LOCATION, TRANSFER_ENCODING},
    },
};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    future::{Future, poll_fn},
    panic::{AssertUnwindSafe, catch_unwind},
    task::Poll,
    time::Duration,
};
use tokio::{runtime::Handle, task::JoinHandle};
use url::Url;

/// URL serializations, redirect joins and stage bookkeeping, in addition to
/// the transport library envelope. DATA keeps its own full-capacity charge.
pub const STAGE_ENVELOPE: u64 = 128 * 1024;

pub trait Body {
    fn status(&self) -> u16;
    fn headers(&self) -> &HeaderMap;
    fn data(
        &mut self,
        credit: &mut impl FnMut(usize) -> Result<(), Error>,
    ) -> impl Future<Output = Result<Option<Data>, Error>>;
}
impl<I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Body for http::Response<I> {
    fn status(&self) -> u16 {
        self.status()
    }
    fn headers(&self) -> &HeaderMap {
        self.headers()
    }
    async fn data(
        &mut self,
        credit: &mut impl FnMut(usize) -> Result<(), Error>,
    ) -> Result<Option<Data>, Error> {
        self.data_with_credit(credit).await
    }
}
/// The test adapter serves exact in-memory schedules, with no DNS fallback.
pub trait Transport {
    type Response: Body;
    fn request(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        request: Request<Payload>,
        timeout: Duration,
        token: OperationToken,
    ) -> impl Future<Output = Result<Self::Response, Error>>;
}
impl Transport for http::Service {
    type Response = http::Response<http::Stream>;
    async fn request(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        request: Request<Payload>,
        timeout: Duration,
        token: OperationToken,
    ) -> Result<Self::Response, Error> {
        self.request_with_token(permit, url, request, timeout, token)
            .await
    }
}
struct Written {
    file: OwnedFile,
    data: Data,
    result: Result<(), Failure>,
    // Keep the write outstanding until the stage receives the join result,
    // including the interval before Tokio publishes the worker's return value.
    _token: OperationToken,
    completed: Duration,
}
struct Pending {
    task: Option<JoinHandle<Written>>,
    value: Option<Written>,
}
impl Pending {
    async fn wait(&mut self) -> Result<(), Failure> {
        poll_fn(|cx| {
            let Some(task) = self.task.as_mut() else {
                return Poll::Ready(Ok(()));
            };
            match std::pin::Pin::new(task).poll(cx) {
                Poll::Ready(result) => {
                    self.task.take();
                    match result {
                        Ok(written) => {
                            self.value = Some(written);
                            Poll::Ready(Ok(()))
                        }
                        Err(_) => Poll::Ready(Err(internal())),
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }
    fn take(&mut self) -> Result<Written, Failure> {
        self.value.take().ok_or_else(internal)
    }
}
/// The pending write is stored here, never solely in the async stack. It owns
/// the original file until receipt; no filesystem-duration mutex is needed.
pub struct Stage<R> {
    pub file: Option<OwnedFile>,
    response: Option<R>,
    write: Option<Pending>,
    _charge: Charge,
    claimed: u64,
}
impl<R: Body> Stage<R> {
    pub fn new(file: OwnedFile, permit: &RequestPermit) -> Result<Self, Failure> {
        let charge = permit.charge(STAGE_ENVELOPE).ok_or_else(memory)?;
        Ok(Self {
            file: Some(file),
            response: None,
            write: None,
            _charge: charge,
            claimed: 0,
        })
    }
    /// Explicit cooperative test/host quiescence only, after holds are released.
    /// Never call this on B9's handover path.
    pub async fn quiesce(&mut self) -> Result<(), Failure> {
        if let Some(task) = self.write.as_mut() {
            task.wait().await?;
            let written = task.take()?;
            self.write.take();
            self.file = Some(written.file);
        }
        self.response.take();
        Ok(())
    }
    /// All received facts are fixed typed values. No URL, header, error string
    /// or redirect history can escape into diagnostics.
    #[allow(clippy::too_many_arguments)]
    pub async fn run<C: Clock, T: Transport<Response = R>>(
        &mut self,
        bundle_index: u32,
        bundle: BundleRef<'_>,
        m: u64,
        policy: &Http,
        values: &impl Values,
        transport: &T,
        permit: &mut RequestPermit,
        pool: &Handle,
        cx: &Context<C>,
    ) -> StageEnd {
        let mut end = StageEnd {
            bundle: bundle_index,
            failure: None,
            integrity: None,
            facts: None,
            claimed_bytes: 0,
        };
        let mut refused = false;
        end.failure = {
            let mut future = std::pin::pin!(self.download(
                bundle, m, policy, values, transport, permit, pool, cx, &mut end,
            ));
            poll_fn(|task| {
                // A current-thread runtime may poll other stages between our
                // polls. Only this synchronous poll belongs to our window.
                let before = crate::alloc::refusals();
                let result = future.as_mut().poll(task);
                refused |= crate::alloc::refusals() != before;
                result
            })
            .await
            .err()
        };
        if refused {
            let mut failure = match end.failure {
                Some(Failure::Run(f)) => Some(f),
                _ => None,
            };
            RunFailure::reduce(&mut failure, RunFailure::new(ReasonCode::HostMemory));
            end.failure = failure.map(Failure::Run);
        }
        if matches!(end.failure, Some(Failure::Data(_))) {
            if let Err(f) = cx.check() {
                end.failure = Some(f);
            } else {
                self.response.take();
                if let Some(file) = &mut self.file {
                    if let Err(e) = file.remove() {
                        end.failure = Some(e.into());
                    }
                } else {
                    end.failure = Some(internal());
                }
            }
        }
        if let Some(Failure::Run(failure)) = end.failure {
            cx.control.request_stop(failure, Culprit::None);
        }
        if !matches!(
            end.failure,
            None | Some(Failure::Data(DataFailure::IntegrityMismatch))
        ) {
            end.facts = None;
            end.integrity = None;
        }
        end.claimed_bytes = self.file.as_ref().map_or(self.claimed, OwnedFile::claimed);
        end
    }
    #[allow(clippy::too_many_arguments)]
    async fn download<C: Clock, T: Transport<Response = R>>(
        &mut self,
        bundle: BundleRef<'_>,
        m: u64,
        policy: &Http,
        values: &impl Values,
        transport: &T,
        permit: &mut RequestPermit,
        pool: &Handle,
        cx: &Context<C>,
        end: &mut StageEnd,
    ) -> Result<(), Failure> {
        cx.check()?;
        if bundle.size().is_some_and(|n| n > m) {
            return Err(limit(m));
        }
        if self
            .file
            .as_ref()
            .is_none_or(|f| f.length() != 0 || f.claimed() != 0)
            || self.write.is_some()
        {
            return Err(internal());
        }
        let original = bundle.url().ok_or_else(internal)?;
        let mut attempt = 0;
        loop {
            cx.check()?;
            cx.wait(poll_fn(|task| permit.poll_available(task))).await?;
            let token = permit.begin(Operation::Lookup).ok_or_else(internal)?;
            cx.check()?; // after actual acquisition, immediately before this try
            let start = cx.clock.now();
            self.file
                .as_mut()
                .ok_or_else(internal)?
                .reserve(bundle.size().unwrap_or(0))?;
            cx.check()?;
            attempt += 1;
            self.claimed = self.file.as_ref().map_or(0, OwnedFile::claimed);
            let mut time = BodyTime::new(
                start,
                Duration::from_millis(u64::from(policy.idle_timeout_ms)),
                policy.min_rate_bytes,
            );
            let mut after = super::retry::After::default();
            let result = self
                .try_download(
                    bundle, m, policy, values, transport, permit, pool, cx, original, token,
                    &mut time, &mut after, end,
                )
                .await;
            let Err(failure) = result else {
                return Ok(());
            };
            let Failure::Data(DataFailure::DownloadFailed(detail)) = failure else {
                return Err(failure);
            };
            cx.check()?;
            self.response.take(); // close before reset, backoff or another hop
            if attempt >= policy.attempts || !super::retry::enabled(policy, detail) {
                return Err(failure);
            }
            cx.check()?;
            let wait = super::retry::backoff(
                policy,
                attempt,
                after.delay(cx.clock.wall(), policy.backoff_max_ms),
            );
            let until = cx.clock.now().saturating_add(wait);
            if cx.cutoff.is_some_and(|cutoff| until >= cutoff) {
                return Err(Failure::Stopped);
            }
            self.file
                .as_mut()
                .ok_or_else(internal)?
                .reset(&|| cx.control.stopped())?;
            cx.check()?;
            cx.wait(cx.clock.sleep_until(until)).await?;
            cx.check()?;
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn try_download<C: Clock, T: Transport<Response = R>>(
        &mut self,
        bundle: BundleRef<'_>,
        m: u64,
        policy: &Http,
        values: &impl Values,
        transport: &T,
        permit: &mut RequestPermit,
        pool: &Handle,
        cx: &Context<C>,
        original: &str,
        token: OperationToken,
        time: &mut BodyTime,
        after: &mut super::retry::After,
        end: &mut StageEnd,
    ) -> Result<(), Failure> {
        let mut url = Url::parse(original).map_err(|_| internal())?;
        let mut followed = 0;
        let mut first_token = Some(token);
        loop {
            cx.check()?;
            let token = if let Some(token) = first_token.take() {
                token
            } else {
                body_wait(cx, time, poll_fn(|task| permit.poll_available(task))).await?;
                let token = permit.begin(Operation::Lookup).ok_or_else(internal)?;
                cx.check()?;
                token
            };
            let request = super::headers::prepare(bundle, values, &url).map_err(transport_error)?;
            self.response = Some(
                body_wait(
                    cx,
                    time,
                    transport.request(
                        permit,
                        &url,
                        request,
                        Duration::from_millis(u64::from(policy.connect_timeout_ms)),
                        token,
                    ),
                )
                .await?
                .map_err(transport_error)?,
            );
            let response = self.response.as_ref().ok_or_else(internal)?;
            let status = response.status();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let next = redirect(&url, response.headers(), status, followed, policy.redirects)?;
                cx.check()?;
                self.response.take();
                url = next;
                followed += 1;
            } else {
                *after = super::retry::retry_after(response.headers());
                if status != 200 {
                    return Err(download(Detail::Status(status)));
                }
                if let Some(length) = content_length(response.headers())? {
                    if bundle.size().is_some_and(|n| n != length) {
                        end.integrity = Some(Integrity::Size);
                        return Err(Failure::Data(DataFailure::IntegrityMismatch));
                    }
                    if length > m {
                        return Err(limit(m));
                    }
                }
                break;
            }
        }
        self.receive(bundle, m, permit, pool, cx, time, end).await
    }
    #[allow(clippy::too_many_arguments)]
    async fn receive<C: Clock>(
        &mut self,
        bundle: BundleRef<'_>,
        m: u64,
        permit: &mut RequestPermit,
        pool: &Handle,
        cx: &Context<C>,
        time: &mut BodyTime,
        end: &mut StageEnd,
    ) -> Result<(), Failure> {
        let mut hash = Sha256::new();
        let mut length = 0_u64;
        loop {
            let response = self.response.as_mut().ok_or_else(internal)?;
            let received = {
                let time = RefCell::new(&mut *time);
                let mut credit = |count| {
                    let mut time = time.borrow_mut();
                    let now = cx.clock.now();
                    if !time.advance(now) {
                        return Err(Error::transport(Detail::Timeout));
                    }
                    time.data(now, count);
                    Ok(())
                };
                body_wait_shared(cx, &time, response.data(&mut credit)).await?
            };
            let Some(data) = received.map_err(transport_error)? else {
                break;
            };
            let count = data.as_slice().len();
            if count == 0 {
                continue;
            }
            let next = length.checked_add(count as u64).ok_or_else(|| limit(m))?;
            if next > m {
                return Err(limit(m));
            }
            if bundle.size().is_some_and(|n| next > n) {
                end.integrity = Some(Integrity::Size);
                return Err(Failure::Data(DataFailure::IntegrityMismatch));
            }
            let data = self.persist(data, length, permit, pool, cx, time).await?;
            hash.update(data.as_slice());
            length = next;
        }
        cx.check()?;
        self.response.take();
        let facts = Facts {
            size: length,
            sha256: hash.finalize().into(),
        };
        end.facts = Some(facts);
        end.integrity = if bundle.size().is_some_and(|n| n != length) {
            Some(Integrity::Size)
        } else if bundle
            .sha256()
            .is_some_and(|text| !digest_matches(text, facts.sha256))
        {
            Some(Integrity::Sha256)
        } else {
            None
        };
        if end.integrity.is_some() {
            return Err(Failure::Data(DataFailure::IntegrityMismatch));
        }
        cx.check()?;
        self.file
            .as_mut()
            .ok_or_else(internal)?
            .seal(&|| cx.control.stopped())?;
        Ok(())
    }
    async fn persist<C: Clock>(
        &mut self,
        data: Data,
        length: u64,
        permit: &mut RequestPermit,
        pool: &Handle,
        cx: &Context<C>,
        time: &mut BodyTime,
    ) -> Result<Data, Failure> {
        let next = length
            .checked_add(data.as_slice().len() as u64)
            .ok_or_else(internal)?;
        cx.check()?;
        let file = self.file.as_mut().ok_or_else(internal)?;
        file.reserve(next)?;
        self.claimed = file.claimed();
        cx.check()?;
        if !time.advance(cx.clock.now()) {
            return Err(download(Detail::Timeout));
        }
        let token = permit.begin(Operation::Write).ok_or_else(internal)?;
        let mut file = self.file.take().ok_or_else(internal)?;
        let control = cx.control.clone();
        let clock = cx.clock.clone();
        let start = cx.clock.now();
        let task = pool.spawn_blocking(move || {
            let before = crate::alloc::refusals();
            let mut result = catch_unwind(AssertUnwindSafe(|| {
                file.write_at_reserved(data.as_slice(), length, &|| control.stopped())
            }))
            .map_err(|_| internal())
            .and_then(|r| r.map_err(Failure::from));
            if crate::alloc::refusals() != before {
                let mut failure = match result {
                    Err(Failure::Run(f)) => Some(f),
                    _ => None,
                };
                RunFailure::reduce(&mut failure, RunFailure::new(ReasonCode::HostMemory));
                result = Err(failure.map_or_else(memory, Failure::Run));
            }
            if let Err(Failure::Run(f)) = result {
                control.request_stop(f, Culprit::None);
            }
            Written {
                file,
                data,
                result,
                _token: token,
                completed: clock.now(),
            }
        });
        self.write = Some(Pending {
            task: Some(task),
            value: None,
        });
        cx.wait(self.write.as_mut().ok_or_else(internal)?.wait())
            .await??;
        let written = self.write.as_mut().ok_or_else(internal)?.take()?;
        self.write.take();
        self.file = Some(written.file);
        written.result?;
        time.pause(written.completed.saturating_sub(start));
        Ok(written.data)
    }
}
async fn body_wait<C: Clock, F: Future>(
    cx: &Context<C>,
    time: &mut BodyTime,
    future: F,
) -> Result<F::Output, Failure> {
    body_wait_shared(cx, &RefCell::new(time), future).await
}
async fn body_wait_shared<C: Clock, F: Future>(
    cx: &Context<C>,
    time: &RefCell<&mut BodyTime>,
    future: F,
) -> Result<F::Output, Failure> {
    let mut future = std::pin::pin!(future);
    let mut started = false;
    loop {
        if let Err(failure) = cx.check() {
            if started {
                std::future::pending::<()>().await;
            }
            return Err(failure);
        }
        if !time.borrow_mut().advance(cx.clock.now()) {
            return Err(download(Detail::Timeout));
        }
        started = true;
        let deadline = time.borrow().deadline();
        let value = cx
            .network_wait(super::timers::race(
                cx.clock.as_ref(),
                deadline,
                future.as_mut(),
            ))
            .await?;
        if !time.borrow_mut().advance(cx.clock.now()) {
            return Err(download(Detail::Timeout));
        }
        if let Some(value) = value {
            return Ok(value);
        }
    }
}
fn redirect(
    url: &Url,
    headers: &HeaderMap,
    status: u16,
    followed: u32,
    policy: Redirects,
) -> Result<Url, Failure> {
    let mut locations = headers.get_all(LOCATION).iter();
    let Some(location) = locations.next() else {
        return Err(download(Detail::Status(status)));
    };
    if locations.next().is_some() {
        return Err(download(Detail::Status(status)));
    }
    let refused = || Failure::Data(DataFailure::RedirectRefused(status));
    if policy == Redirects::None || followed >= 5 || location.as_bytes().len() > 16 * 1024 {
        return Err(refused());
    }
    let next = url
        .join(
            location
                .to_str()
                .map_err(|_| download(Detail::Status(status)))?,
        )
        .map_err(|_| download(Detail::Status(status)))?;
    if http::request::validate_url(&next).is_err() || next.origin() != url.origin() {
        return Err(refused());
    }
    Ok(next)
}
/// Match pinned hyper's framing: TE overrides CL, otherwise every trimmed
/// comma-separated value in every field must be digits and equal.
fn content_length(headers: &HeaderMap) -> Result<Option<u64>, Failure> {
    if headers.contains_key(TRANSFER_ENCODING) {
        return Ok(None);
    }
    let invalid = || download(Detail::NoResponse);
    let mut length = None;
    for field in headers.get_all(CONTENT_LENGTH) {
        for value in field.to_str().map_err(|_| invalid())?.split(',') {
            let value = value.trim();
            if value.is_empty() {
                return Err(invalid());
            }
            let mut n = 0_u64;
            for byte in value.bytes() {
                if !byte.is_ascii_digit() {
                    return Err(invalid());
                }
                n = n
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(u64::from(byte - b'0')))
                    .ok_or_else(invalid)?;
            }
            if length.is_some_and(|previous| previous != n) {
                return Err(invalid());
            }
            length = Some(n);
        }
    }
    Ok(length)
}
fn digest_matches(text: &str, digest: [u8; 32]) -> bool {
    text.len() == 64
        && text
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .zip(digest)
            .all(|(pair, byte)| {
                let hex = b"0123456789abcdef";
                Some(&pair[0]) == hex.get(usize::from(byte >> 4))
                    && Some(&pair[1]) == hex.get(usize::from(byte & 15))
            })
}
fn download(detail: Detail) -> Failure {
    Failure::Data(DataFailure::DownloadFailed(detail))
}
fn limit(m: u64) -> Failure {
    Failure::Data(DataFailure::InputBytes(m))
}
fn internal() -> Failure {
    Failure::Run(RunFailure::new(ReasonCode::Internal))
}
fn memory() -> Failure {
    Failure::Run(RunFailure::new(ReasonCode::HostMemory))
}
fn transport_error(error: Error) -> Failure {
    match error {
        Error::Transport(f) => download(f.detail),
        Error::Memory => memory(),
        Error::InvalidRequest | Error::Busy | Error::Pool => internal(),
    }
}
