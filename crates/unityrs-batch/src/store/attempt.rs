//! A store request's tries (`docs/BATCH_SPEC.md` 1.4, 1.6; `docs/plans/
//! batch.md` 2.9). Each try checks the stop and the cutoff, counts itself
//! unsettled when its kind is counted, runs its `object_store` call and every
//! body read after it under one `timeout_ms` timer, inside a task-local
//! [`TrySlot`] the connector records into, and is classified by 1.6's table.
//! A retried try waits by 1.4's backoff, raised by `Retry-After`, and none
//! starts when its wait would pass the cutoff. After a stop nothing is
//! retried; a try a stop abandons is never polled again and stays counted.
use super::{
    Kind,
    classify::{self, Action, Ended, Facts, Verdict},
    scan::Code,
};
use crate::{
    control::Control,
    http::{Detail, permit::RequestPermit},
    spec::model::Http,
    stage::retry,
};
use std::{
    future::Future,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::time::Instant;

tokio::task_local! {
    static SLOT: Arc<TrySlot>;
}

/// A failure of the engine's own, not the store's: the run fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// An assumption the connector checks broke: a second request in one
    /// call, a call outside a try, a request whose URI the `url` crate
    /// would not send byte for byte, another origin, a header the engine
    /// does not expect (`internal`).
    Internal,
    /// A charge or an allocation was refused.
    Memory,
}

/// How a body ended once its status arrived (section 10's `truncated`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyFault {
    /// The transport failed mid-body.
    Transport(Detail),
    /// A 2xx body passed its kind's cap.
    OverCap,
    /// A LIST page broke the tag scan's limits.
    Scan,
}

/// What one try recorded. It holds no header, URL or body text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Record {
    pub connected: bool,
    pub status: Option<u16>,
    pub(crate) retry_after: retry::After,
    pub code: Code,
    /// The transport's failure before a status.
    pub detail: Option<Detail>,
    pub body: Option<BodyFault>,
    pub fault: Option<Fault>,
    /// Requests the try's `object_store` call made.
    pub calls: u8,
}

impl Record {
    #[must_use]
    pub const fn facts(&self) -> Facts {
        Facts {
            connected: self.connected,
            status: self.status,
            code: self.code,
        }
    }
}

struct State {
    kind: Kind,
    permit: Option<RequestPermit>,
    record: Record,
}

/// A request's slot, shared by its tries: the request kind, the record of
/// the try in flight and custody of the request's permit, which the
/// connector borrows for its one call behind a guard that gives it back
/// even when the timer drops the call.
pub struct TrySlot {
    connected: AtomicBool,
    state: Mutex<State>,
}

impl TrySlot {
    /// One per request, allocated once for all its tries (`C_net`).
    #[must_use]
    pub fn new(permit: RequestPermit, kind: Kind) -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(false),
            state: Mutex::new(State {
                kind,
                permit: Some(permit),
                record: Record::default(),
            }),
        })
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Starts a try of `kind` with an empty record.
    fn begin(&self, kind: Kind) {
        self.connected.store(false, Ordering::Release);
        let mut state = self.lock();
        state.kind = kind;
        state.record = Record::default();
    }
    /// The record so far.
    #[must_use]
    pub fn record(&self) -> Record {
        let mut record = self.lock().record;
        record.connected = self.connected.load(Ordering::Acquire);
        record
    }
    /// The request's permit back, once no call holds it.
    #[must_use]
    pub fn take_permit(&self) -> Option<RequestPermit> {
        self.lock().permit.take()
    }
    /// The slot of the try this task is running, if any.
    pub(crate) fn current() -> Option<Arc<Self>> {
        SLOT.try_with(Arc::clone).ok()
    }
    /// The connector's call starts: the try's one request. A second, or a
    /// call with no permit, is the engine's fault.
    pub(crate) fn enter(self: &Arc<Self>) -> Result<(Kind, PermitGuard), Fault> {
        let mut state = self.lock();
        state.record.calls = state.record.calls.saturating_add(1);
        let permit = if state.record.calls == 1 {
            state.permit.take()
        } else {
            None
        };
        let Some(permit) = permit else {
            state.record.fault = Some(Fault::Internal);
            return Err(Fault::Internal);
        };
        Ok((
            state.kind,
            PermitGuard {
                slot: Arc::clone(self),
                permit: Some(permit),
            },
        ))
    }
    pub(crate) fn connected_flag(&self) -> &AtomicBool {
        &self.connected
    }
    pub(crate) fn update(&self, f: impl FnOnce(&mut Record)) {
        f(&mut self.lock().record);
    }
    pub(crate) fn fail(&self, fault: Fault) {
        self.update(|r| {
            r.fault.get_or_insert(fault);
        });
    }
}

/// The connector's custody of the permit for one call.
pub(crate) struct PermitGuard {
    slot: Arc<TrySlot>,
    permit: Option<RequestPermit>,
}
impl PermitGuard {
    pub(crate) fn permit(&mut self) -> Option<&mut RequestPermit> {
        self.permit.as_mut()
    }
}
impl Drop for PermitGuard {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.slot.lock().permit = Some(permit);
        }
    }
}

/// A store request's tries: the sink's `attempts` and `timeout_ms`, the
/// `http` backoff, and the stop cutoff (the deadline less the grace).
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub attempts: u32,
    pub timeout: Duration,
    pub backoff_initial_ms: u32,
    pub backoff_max_ms: u32,
    pub cutoff: Option<Instant>,
}

impl Policy {
    fn backoff(&self, retry: u32, after: Duration) -> Duration {
        let http = Http {
            backoff_initial_ms: self.backoff_initial_ms,
            backoff_max_ms: self.backoff_max_ms,
            ..Http::default()
        };
        retry::backoff(&http, retry, after)
    }
    /// The wait before the try after the `tried`-th, or none when it would
    /// start past the cutoff.
    #[must_use]
    pub fn next_wait(
        &self,
        tried: u32,
        verdict: Verdict,
        record: &Record,
        now: Instant,
        wall: SystemTime,
    ) -> Option<Duration> {
        let after = match verdict.action {
            Action::Retry { retry_after: true } => {
                record.retry_after.delay(wall, self.backoff_max_ms)
            }
            _ => Duration::ZERO,
        };
        let wait = self.backoff(tried, after);
        match self.cutoff {
            Some(cutoff) if now.checked_add(wait).is_none_or(|start| start >= cutoff) => None,
            _ => Some(wait),
        }
    }
    fn past_cutoff(&self) -> bool {
        self.cutoff.is_some_and(|cutoff| Instant::now() >= cutoff)
    }
}

/// How a request's tries ended.
#[derive(Debug)]
pub struct Done<T> {
    /// The last try's value, when its call returned one.
    pub value: Option<T>,
    /// The last try's row, or `None` when a stop or the cutoff came before
    /// any try ran.
    pub verdict: Option<Verdict>,
    /// The last try's record.
    pub record: Record,
    /// Tries made.
    pub tries: u32,
    /// A stop or the cutoff ended the tries.
    pub stopped: bool,
}

/// The tries' shared state: the run's stop and its `unsettled_tries` count.
pub struct Tries<'a> {
    pub policy: Policy,
    pub control: &'a dyn Control,
    pub unsettled: &'a AtomicU64,
}

impl Tries<'_> {
    /// Runs `op` (its argument the try's number, from 1) once per try of
    /// `kind` in `slot`, until a try is not retried, the tries run out, or
    /// a stop or the cutoff comes. A fault fails the run, its try left
    /// counted.
    pub async fn run<T, F, Fut>(
        &self,
        slot: &Arc<TrySlot>,
        kind: Kind,
        mut op: F,
    ) -> Result<Done<T>, Fault>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T, Ended>>,
    {
        let mut done = Done {
            value: None,
            verdict: None,
            record: Record::default(),
            tries: 0,
            stopped: false,
        };
        loop {
            if self.control.stopped() || self.policy.past_cutoff() {
                done.stopped = true;
                return Ok(done);
            }
            done.tries += 1;
            if kind.counted() {
                self.unsettled.fetch_add(1, Ordering::AcqRel);
            }
            slot.begin(kind);
            let result = SLOT
                .scope(
                    Arc::clone(slot),
                    tokio::time::timeout(self.policy.timeout, op(done.tries)),
                )
                .await;
            let (value, ended) = match result {
                Ok(Ok(value)) => (Some(value), Ended::Ok),
                Ok(Err(ended)) => (None, ended),
                Err(_) => (None, Ended::TimedOut),
            };
            let record = slot.record();
            if let Some(fault) = record.fault {
                return Err(fault);
            }
            if ended == Ended::Ok && record.calls == 0 {
                // A value no request answered: not a try 1.6 can classify.
                return Err(Fault::Internal);
            }
            let verdict = classify::classify(kind, done.tries > 1, &record.facts(), ended);
            if verdict.settled && kind.counted() {
                self.unsettled.fetch_sub(1, Ordering::AcqRel);
            }
            done.value = value;
            done.verdict = Some(verdict);
            done.record = record;
            if !verdict.retried() || done.tries >= self.policy.attempts || self.control.stopped() {
                return Ok(done);
            }
            let Some(wait) = self.policy.next_wait(
                done.tries,
                verdict,
                &record,
                Instant::now(),
                SystemTime::now(),
            ) else {
                return Ok(done);
            };
            tokio::time::sleep(wait).await;
        }
    }
}
