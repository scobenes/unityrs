//! `docs/BATCH_SPEC.md` 1.6's table: a try's class, by the first row that
//! matches, from what its [`TrySlot`](super::attempt::TrySlot) recorded and
//! how the `object_store` call ended. Pure: no clock, no IO.
use super::{Kind, scan::Code};

/// How a try's call ended, as the try wrapper saw it. Only a 2xx reads it:
/// any other status takes its status's row however its body ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ended {
    /// The call returned its value, which the caller kept.
    Ok,
    /// The call returned a value the engine refuses (an `ETag` it keeps
    /// over 100 bytes): a 2xx the call returns as an error.
    Rejected,
    /// The call returned an error.
    Error,
    /// The sink's `timeout_ms` fired and dropped the try.
    TimedOut,
}

/// What the table makes of a try.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// The request succeeded.
    Success,
    /// Retried while tries remain; `retry_after` when the response's
    /// `Retry-After` raises the wait.
    Retry { retry_after: bool },
    /// A head write's precondition failed: read the head back (7.2).
    ReadBack,
    /// A head read found none (7.2).
    NoHead,
    /// `NoSuchUpload` on a retried complete: HEAD the key (E37).
    HeadCheck,
    /// `ExpiredToken`.
    CredentialsExpired,
    /// Any other 3xx or 4xx.
    PublishFailed,
    /// Anything else: neither settled nor retried.
    Unsettled,
}

/// A try's row: whether the store may still commit it, and what follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verdict {
    /// The try is settled: the store will not commit it later.
    pub settled: bool,
    pub action: Action,
}

impl Verdict {
    const fn new(settled: bool, action: Action) -> Self {
        Self { settled, action }
    }
    /// Whether the table retries the try while tries remain.
    #[must_use]
    pub const fn retried(self) -> bool {
        matches!(self.action, Action::Retry { .. })
    }
}

/// What a try's slot recorded that the table reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Facts {
    /// The connection was up (1.4).
    pub connected: bool,
    /// The response's status, once its head arrived.
    pub status: Option<u16>,
    /// The S3 error code its body named.
    pub code: Code,
}

/// 1.6's table. `retried` says the try is not its request's first (only a
/// retried complete's `NoSuchUpload` reads it). Preflight's bucket proof
/// takes any 2xx as proof, whatever its body, and an abort's
/// `NoSuchUpload` is a successful abort: their own outcomes, before the
/// table's rows; their callers read only its first two columns.
#[must_use]
pub fn classify(kind: Kind, retried: bool, facts: &Facts, ended: Ended) -> Verdict {
    let status = facts.status;
    if kind == Kind::ListProof && status.is_some_and(|s| (200..300).contains(&s)) {
        return Verdict::new(true, Action::Success);
    }
    let client_error = status.is_some_and(|s| (400..500).contains(&s));
    if kind == Kind::Abort && client_error && facts.code.is("NoSuchUpload") {
        return Verdict::new(true, Action::Success);
    }
    let Some(status) = status else {
        // Rows 1 and 2: nothing was sent before the connection was up; any
        // other end before a status may have reached the store.
        return Verdict::new(!facts.connected, Action::Retry { retry_after: false });
    };
    let retry_after = Action::Retry { retry_after: true };
    match status {
        500..=599 => Verdict::new(false, retry_after),
        200..=299 if ended != Ended::Ok => Verdict::new(false, retry_after),
        200..=299 => Verdict::new(true, Action::Success),
        408 | 429 => Verdict::new(true, retry_after),
        304 | 409 | 412 if matches!(kind, Kind::HeadWrite { .. }) => {
            Verdict::new(true, Action::ReadBack)
        }
        404 if kind == (Kind::HeadWrite { if_match: true }) => Verdict::new(true, Action::ReadBack),
        404 if kind == Kind::HeadRead => Verdict::new(true, Action::NoHead),
        400..=499 if kind == Kind::Complete && retried && facts.code.is("NoSuchUpload") => {
            Verdict::new(true, Action::HeadCheck)
        }
        400..=499 if facts.code.is("ExpiredToken") => {
            Verdict::new(true, Action::CredentialsExpired)
        }
        300..=499 => Verdict::new(true, Action::PublishFailed),
        _ => Verdict::new(false, Action::Unsettled),
    }
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
