//! The Environment's own locks, taken one at a time (plan section 3.16: no
//! lock is taken while another is held, the budget's aside). Every guard on
//! one of them is a [`Held`], made by [`lock`], [`read`] or [`write`]: the
//! registry's (over its keys, its lists of dropped slots, the container
//! index generation and `RegistryInner.watched`, the table of the walks'
//! flags under registry keys), the `files` listing's (over the list, its
//! indexes and `Files.watched`, the table of the walks' flags under `files`
//! keys), a dependency's `open` and the LRU's list. In a test build each thread
//! counts the guards it holds, and taking one while it holds another
//! panics, so every unit test that runs an Environment checks the rule
//! (review round 6: a dependency's reopen took its meter's lock while it
//! held its `open` lock, and nothing would have told). The budget's lock,
//! which a charge may take under a core lock, is not one of them.
//!
//! In a test build [`after_each_release`] runs a check on this thread right
//! as each guard is released, with nothing between the release and the
//! check, so a test sees what another thread can see the moment a lock is
//! free: a step that must be one step under a lock is pinned there (W1 and
//! P1, plan section 3.16; review round 15: a test hook placed after the lock
//! passed a notify moved out of it, and a marker the publishing code set
//! itself would have moved out with it).

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
    /// What runs on this thread as each of its guards is released.
    static AFTER: std::cell::RefCell<Option<AfterRelease>> =
        const { std::cell::RefCell::new(None) };
}

/// A check [`after_each_release`] runs.
#[cfg(test)]
pub(crate) type AfterRelease = std::rc::Rc<dyn Fn()>;

/// Runs `check` on this thread right after each guard this thread releases,
/// its lock free and nothing run since (a guard's `Count` is the field that
/// drops right after its lock's); `None` takes it off. The check may take
/// locks: their releases do not run it again.
#[cfg(test)]
pub(crate) fn after_each_release(check: Option<AfterRelease>) {
    AFTER.with(|a| *a.borrow_mut() = check);
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
        // A guard dropped while its thread unwinds a panic checks nothing:
        // a second panic would abort the test binary.
        if std::thread::panicking() {
            return;
        }
        let check = AFTER.with(|a| a.borrow_mut().take());
        if let Some(check) = check {
            check();
            AFTER.with(|a| *a.borrow_mut() = Some(check));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, RwLock};

    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use super::{after_each_release, lock, read, write};

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

    /// The check runs right after each release, the lock free, and what it
    /// takes itself does not run it again; a release a panic unwinds runs
    /// none (a second panic would abort the test binary); off, it runs no
    /// more.
    #[test]
    fn a_check_runs_after_each_release_with_the_lock_free() {
        let l = Rc::new(RwLock::new(1));
        let seen = Rc::new(Cell::new(0));
        after_each_release(Some({
            let (l, seen) = (l.clone(), seen.clone());
            Rc::new(move || {
                assert!(l.try_write().is_ok(), "the lock is still held");
                seen.set(seen.get() + *read(&l));
            })
        }));
        *write(&l) = 2;
        assert_eq!(seen.get(), 2, "the check saw the release's value, once");
        drop(read(&l));
        assert_eq!(seen.get(), 4);
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _held = read(&l);
            panic!("unwinds the guard");
        }));
        assert!(unwound.is_err());
        assert_eq!(seen.get(), 4, "a release a panic unwound ran the check");
        after_each_release(None);
        drop(read(&l));
        assert_eq!(seen.get(), 4);
    }
}
