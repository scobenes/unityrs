//! The identity of a vector-growth invocation, for tests that must tell a
//! retry inside its ladder from a later call at the same source location.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static CURRENT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The calling thread's current vector-growth invocation, or none outside
/// a growth call. Each invocation has a distinct id; its ladder's retries
/// share it. Nested calls restore the enclosing invocation on return.
#[must_use]
pub fn current_growth() -> Option<u64> {
    CURRENT.try_with(Cell::get).ok().flatten()
}

/// Keeps an invocation's id active until the growth returns or unwinds.
pub(crate) struct Growing(Option<u64>);

impl Growing {
    pub(crate) fn enter() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(CURRENT.try_with(|c| c.replace(Some(id))).ok().flatten())
    }
}

impl Drop for Growing {
    fn drop(&mut self) {
        let _ = CURRENT.try_with(|c| c.set(self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::{Growing, current_growth};

    #[test]
    fn nested_growth_restores_its_enclosing_invocation() {
        assert_eq!(current_growth(), None);
        let outer = Growing::enter();
        let id = current_growth();
        assert!(id.is_some());
        {
            let _inner = Growing::enter();
            assert!(current_growth().is_some());
            assert_ne!(current_growth(), id);
        }
        assert_eq!(current_growth(), id);
        drop(outer);
        assert_eq!(current_growth(), None);
    }
}
