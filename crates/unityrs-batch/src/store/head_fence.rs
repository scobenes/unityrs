//! Retained facts for one logical head write, independent of the try slot.
use super::{
    attempt::{Observer, Record},
    classify::{Action, Verdict},
    etag::Etag,
};
use std::cell::Cell;
#[allow(clippy::struct_excessive_bools)] // Independent facts from spec 7.4, not exclusive states.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct History {
    pub attempted: bool,
    pub succeeded: bool,
    pub in_flight: bool,
    pub unsettled: bool,
    pub retryable_4xx: bool,
}
impl History {
    /// Spec 7.4's second row wins over its first after a stop. A read-back
    /// never changes these write facts; an abandoned try remains in flight.
    #[must_use]
    pub const fn needs_final_after_stop(self) -> bool {
        !self.succeeded && (self.in_flight || self.unsettled || self.retryable_4xx)
    }
}
/// One per logical write, retained outside cancellation. Initialize the final
/// write's fence from running's acknowledged `ETag` or its last precondition.
#[derive(Clone, Debug, Default)]
pub struct Fence {
    last: Cell<Option<Etag>>,
    history: Cell<History>,
}
impl Fence {
    #[must_use]
    pub const fn new(initial: Option<Etag>) -> Self {
        Self {
            last: Cell::new(initial),
            history: Cell::new(History {
                attempted: false,
                succeeded: false,
                in_flight: false,
                unsettled: false,
                retryable_4xx: false,
            }),
        }
    }
    #[must_use]
    pub fn last(&self) -> Option<Etag> {
        self.last.get()
    }
    #[must_use]
    pub fn history(&self) -> History {
        self.history.get()
    }
    pub(super) fn observer(&self, precondition: Option<Etag>) -> WriteObserver<'_> {
        WriteObserver {
            fence: self,
            precondition,
        }
    }
    pub(super) fn acknowledge(&self) {
        let mut history = self.history.get();
        history.succeeded = true;
        self.history.set(history);
    }
}
pub(super) struct WriteObserver<'a> {
    fence: &'a Fence,
    precondition: Option<Etag>,
}
impl Observer for WriteObserver<'_> {
    fn started(&self) {
        self.fence.last.set(self.precondition);
        let mut history = self.fence.history.get();
        history.attempted = true;
        history.in_flight = true;
        self.fence.history.set(history);
    }
    fn ended(&self, record: &Record, verdict: Verdict) {
        let mut history = self.fence.history.get();
        history.in_flight = false;
        history.unsettled |= !verdict.settled;
        history.retryable_4xx |= matches!(record.status, Some(408 | 429));
        history.succeeded |= verdict.action == Action::Success;
        self.fence.history.set(history);
    }
}
