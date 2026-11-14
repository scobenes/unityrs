//! Fixed retry classes and capped Retry-After parsing; no rendered peer data.
use crate::{
    http::{Detail, RetryClass},
    spec::model::{Http, TryClass},
};
use hyper::http::{HeaderMap, header::RETRY_AFTER};
use std::time::{Duration, SystemTime};

pub(crate) fn enabled(policy: &Http, detail: Detail) -> bool {
    detail.retry_class().is_some_and(|class| {
        policy.retry_on.contains(match class {
            RetryClass::Connect => TryClass::Connect,
            RetryClass::NoResponse => TryClass::NoResponse,
            RetryClass::Timeout => TryClass::Timeout,
            RetryClass::Truncated => TryClass::Truncated,
            RetryClass::Status5xx => TryClass::Status5xx,
            RetryClass::Status429 => TryClass::Status429,
        })
    })
}
pub(crate) fn backoff(policy: &Http, retry: u32, after: Duration) -> Duration {
    let cap = Duration::from_millis(u64::from(policy.backoff_max_ms));
    Duration::from_millis(
        u64::from(policy.backoff_initial_ms).saturating_mul(
            1_u64
                .checked_shl(retry.saturating_sub(1))
                .unwrap_or(u64::MAX),
        ),
    )
    .max(after)
    .min(cap)
}
/// Preserve absolute dates until the failed try schedules its retry. Only the
/// largest date and delta are needed; neither owns header storage.
#[derive(Default)]
pub(crate) struct After {
    seconds: u64,
    date: Option<SystemTime>,
}
impl After {
    pub fn delay(&self, wall: SystemTime, cap: u32) -> Duration {
        Duration::from_secs(self.seconds)
            .max(
                self.date
                    .and_then(|date| date.duration_since(wall).ok())
                    .unwrap_or_default(),
            )
            .min(Duration::from_millis(u64::from(cap)))
    }
}
pub(crate) fn retry_after(headers: &HeaderMap) -> After {
    let mut after = After::default();
    for value in headers.get_all(RETRY_AFTER) {
        let Ok(text) = value.to_str() else { continue };
        let text = text.trim();
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            let seconds = text.bytes().fold(0_u64, |n, b| {
                n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
            });
            after.seconds = after.seconds.max(seconds);
        } else if let Ok(date) = httpdate::parse_http_date(text) {
            after.date = Some(after.date.map_or(date, |previous| previous.max(date)));
        }
    }
    after
}
