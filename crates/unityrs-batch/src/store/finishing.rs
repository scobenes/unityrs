//! Live B9 budgets around a finisher request, including an already pending
//! lookup/body/backoff. Each request may start one try after a stop; a try
//! already admitted before it is never retried afterward.
use super::{
    attempt::{Observer, Record, Tries},
    classify::Verdict,
    put::Error,
};
use crate::{
    control::{Control, Culprit},
    finish::budget::{Budget, Hard},
    job::RunFailure,
    lifetime::Boot,
};
use std::{
    future::{Future, poll_fn},
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
    time::Duration,
};

pub(super) struct ControlAt<'a> {
    hard: &'a dyn Hard,
    begun: AtomicBool,
}
impl<'a> ControlAt<'a> {
    pub(super) fn new(hard: &'a dyn Hard) -> Self {
        Self {
            hard,
            begun: AtomicBool::new(false),
        }
    }
}
impl Control for ControlAt<'_> {
    fn stopped(&self) -> bool {
        self.begun.load(Ordering::Acquire) && self.hard.hard().is_some()
    }
    fn request_stop(&self, _: RunFailure, _: Culprit) {}
}
pub(super) struct Observed<'a> {
    pub control: &'a ControlAt<'a>,
    pub observer: &'a dyn Observer,
}
impl Observer for Observed<'_> {
    fn started(&self) {
        self.control.begun.store(true, Ordering::Release);
        self.observer.started();
    }
    fn ended(&self, record: &Record, verdict: Verdict) {
        self.observer.ended(record, verdict);
    }
}
pub(super) fn tries<'a>(old: &'a Tries<'a>, control: &'a ControlAt<'a>) -> Tries<'a> {
    Tries {
        policy: old.policy,
        control,
        unsettled: old.unsettled,
    }
}
pub(super) async fn drive<T>(
    budget: Budget<'_>,
    head: bool,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    let mut future = std::pin::pin!(future);
    let mut timer = std::pin::pin!(tokio::time::sleep(Duration::ZERO));
    // A library caller may arm its own State without a process wake. The
    // process path also registers its lifecycle wake before reading hard.
    let mut refresh = std::pin::pin!(tokio::time::sleep(Duration::from_millis(10)));
    let mut programmed = None;
    poll_fn(|cx| {
        while let Poll::Ready(result) = budget.hard.poll_change(cx) {
            if result.is_err() {
                return Poll::Ready(Err(Error::Stopped));
            }
        }
        let cut = if head {
            budget.head_deadline()
        } else {
            budget.row_deadline()
        };
        if let Some(cut) = cut {
            let now = Boot::now().unwrap_or(Boot(u64::MAX));
            if now >= cut {
                return Poll::Ready(Err(Error::Stopped));
            }
            if programmed != Some(cut) {
                timer.as_mut().reset(
                    tokio::time::Instant::now()
                        + Duration::from_millis(cut.0.saturating_sub(now.0)),
                );
                programmed = Some(cut);
            }
        } else if refresh.as_mut().poll(cx).is_ready() {
            refresh
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_millis(10));
            let _ = refresh.as_mut().poll(cx);
        }
        if cut.is_some() && timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Error::Stopped));
        }
        future.as_mut().poll(cx).map(Ok)
    })
    .await
}
