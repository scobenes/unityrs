//! The key [`collect`](super::collect)'s check for a name two inputs give
//! keeps its names under.
//!
//! A [`Name`]'s bytes are private to this module, so the check reaches them
//! only through `Eq` and `Hash`; in tests each call of either is one step
//! of the check's work (`NAME_STEPS`). A hash set hashes a name once to look
//! it up or keep it (and again when it grows) and compares it only with a
//! name of the same hash, so its steps grow with the number of names; a
//! scan compares a name with every name kept, and its steps grow with their
//! square (review round 10: a test that counted the lookups the check
//! reported passed a scan).

use std::hash::{Hash, Hasher};

/// A name as the repeat check keeps it.
pub(super) struct Name(Vec<u8>);

impl Name {
    pub(super) fn new(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        step();
        self.0 == other.0
    }
}

impl Eq for Name {}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        step();
        self.0.hash(state);
    }
}

#[cfg(test)]
std::thread_local! {
    /// The `Eq` and `Hash` calls this thread's names took (the linear-work
    /// test).
    pub(super) static NAME_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts one step (tests only).
#[inline]
fn step() {
    #[cfg(test)]
    NAME_STEPS.with(|s| s.set(s.get().saturating_add(1)));
}
