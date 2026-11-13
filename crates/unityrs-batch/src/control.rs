//! Canonical, synchronous stop seam. B9 owns arbitration and wake ownership.
use crate::job::RunFailure;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Culprit {
    None,
    Input(u32),
    Bundle(u32),
}

/// Implementations publish the canonical stop before `request_stop` returns.
/// A host must also wake the execution's wake descriptor when requesting an
/// external stop. No delayed mirror is used by attempts or commit loads.
pub trait Control: Send + Sync {
    fn stopped(&self) -> bool;
    fn request_stop(&self, failure: RunFailure, culprit: Culprit);
}

/// Minimal cooperative/test implementation; deliberately does no arbitration.
/// B9 supplies a process-wide implementation retaining the reason and culprit.
impl Control for AtomicBool {
    fn stopped(&self) -> bool {
        self.load(Ordering::Acquire)
    }
    fn request_stop(&self, _failure: RunFailure, _culprit: Culprit) {
        self.store(true, Ordering::Release);
    }
}
