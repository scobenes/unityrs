//! The Environment's own locks, taken one at a time (plan section 3.16: no
//! lock is taken while another is held, the budget's aside). Every guard on
//! one of them is a [`Held`], made by [`lock`], [`read`] or [`write`]: the
//! registry's table, a dependency's `open`, the LRU's list, the walks'
//! watch map and the `files` list. In a test build each thread counts the
//! guards it holds, and taking one while it holds another panics, so every
//! unit test that runs an Environment checks the rule (review round 6: a
//! dependency's reopen took its meter's lock while it held its `open`
//! lock, and nothing would have told). The budget's lock, which a charge
//! may take under a core lock, is not one of them.

use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A guard on one of the Environment's locks (module docs): the guard, and
/// in a test build the thread's count of held guards.
pub(crate) struct Held<G> {
    // Dropped first: the lock is released before the count goes down.
    guard: G,
    #[cfg(test)]
    _count: Count,
}

impl<G: Deref> Deref for Held<G> {
    type Target = G::Target;

    fn deref(&self) -> &G::Target {
        &self.guard
    }
}

impl<G: DerefMut> DerefMut for Held<G> {
    fn deref_mut(&mut self) -> &mut G::Target {
        &mut self.guard
    }
}

impl<G> Held<G> {
    /// Takes the lock `take` takes, after the thread's count says it holds
    /// none of the Environment's (module docs).
    #[track_caller]
    fn take(take: impl FnOnce() -> G) -> Self {
        #[cfg(test)]
        let count = Count::new();
        Self {
            guard: take(),
            #[cfg(test)]
            _count: count,
        }
    }
}

/// `m`'s lock, poisoned or not.
#[track_caller]
pub(crate) fn lock<T>(m: &Mutex<T>) -> Held<MutexGuard<'_, T>> {
    Held::take(|| m.lock().unwrap_or_else(PoisonError::into_inner))
}

/// `l`'s read lock, poisoned or not.
#[track_caller]
pub(crate) fn read<T>(l: &RwLock<T>) -> Held<RwLockReadGuard<'_, T>> {
    Held::take(|| l.read().unwrap_or_else(PoisonError::into_inner))
}

/// `l`'s write lock, poisoned or not.
#[track_caller]
pub(crate) fn write<T>(l: &RwLock<T>) -> Held<RwLockWriteGuard<'_, T>> {
    Held::take(|| l.write().unwrap_or_else(PoisonError::into_inner))
}

#[cfg(test)]
thread_local! {
    /// The Environment's locks this thread holds.
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One held guard, counted on its thread.
#[cfg(test)]
struct Count;

#[cfg(test)]
impl Count {
    #[track_caller]
    fn new() -> Self {
        HELD.with(|held| {
            assert_eq!(
                held.get(),
                0,
                "an Environment lock taken while another is held (plan section 3.16)"
            );
            held.set(1);
        });
        Self
    }
}

#[cfg(test)]
impl Drop for Count {
    fn drop(&mut self) {
        HELD.with(|held| held.set(held.get().saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, RwLock};

    use super::{lock, read, write};

    /// Each lock alone, one after another, is fine; one taken while another
    /// is held panics.
    #[test]
    fn a_lock_taken_while_another_is_held_panics() {
        let (m, l) = (Mutex::new(1), RwLock::new(2));
        let one = *lock(&m);
        assert_eq!(one + *read(&l), 3);
        *write(&l) = 3;
        let nested = std::panic::catch_unwind(|| {
            let _m = lock(&m);
            drop(read(&l));
        });
        assert!(nested.is_err(), "the nested lock was not caught");
        // The count is back to none after the panic unwound the guard.
        assert_eq!(*read(&l), 3);
    }
}
