//! Fallible strong-only shared storage. Fixed phase/dispatcher envelopes cover
//! its small header and payload; unlike `Arc::new`, refusal returns to the caller.
use std::{
    io,
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering, fence},
};
#[repr(C)]
struct Inner<T> {
    refs: AtomicUsize,
    value: T,
}
pub struct Shared<T>(NonNull<Inner<T>>);
// SAFETY: sharing only exposes &T, and the final owner may drop T on any thread.
unsafe impl<T: Send + Sync> Send for Shared<T> {}
// SAFETY: the reference count is atomic and shared access requires T: Sync.
unsafe impl<T: Send + Sync> Sync for Shared<T> {}
impl<T> Shared<T> {
    pub fn new(value: T) -> io::Result<Self> {
        let inner = crate::alloc::try_box(Inner {
            refs: AtomicUsize::new(1),
            value,
        })?;
        Ok(Self(NonNull::from(Box::leak(inner))))
    }
}
impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        // As for Arc, overflow is unreachable within addressable storage.
        if self.inner().refs.fetch_add(1, Ordering::Relaxed) > isize::MAX as usize {
            std::process::abort();
        }
        Self(self.0)
    }
}
impl<T> Shared<T> {
    #[must_use]
    pub fn strong_count(this: &Self) -> usize {
        this.inner().refs.load(Ordering::Acquire)
    }
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if this.inner().refs.load(Ordering::Acquire) != 1 {
            return None;
        }
        // SAFETY: the unique strong reference is mutably borrowed; Shared has
        // no weak references, so no other accessor or new owner can exist.
        Some(unsafe { &mut this.0.as_mut().value })
    }
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        if this
            .inner()
            .refs
            .compare_exchange(1, 0, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(this);
        }
        let ptr = this.0.as_ptr();
        std::mem::forget(this);
        // SAFETY: we consumed the only strong owner. Moving value out of the
        // reconstructed allocation frees its header without dropping T twice.
        let inner = unsafe { Box::from_raw(ptr) };
        Ok(inner.value)
    }
    fn inner(&self) -> &Inner<T> {
        // SAFETY: this strong reference keeps the initialized allocation alive.
        unsafe { self.0.as_ref() }
    }
}
impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner().value
    }
}
impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        if self.inner().refs.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            // SAFETY: this was the last strong owner, with no weak pointers;
            // acquire pairs with releases by all earlier owners. Reconstruct
            // exactly the Box that Shared::new leaked, then drop/free it once.
            drop(unsafe { Box::from_raw(self.0.as_ptr()) });
        }
    }
}

/// A reserved strong owner, initialized exactly once before it can be shared.
pub struct Slot<T>(Shared<std::mem::MaybeUninit<T>>);
impl<T> Slot<T> {
    pub fn reserve() -> io::Result<Self> {
        Shared::new(std::mem::MaybeUninit::uninit()).map(Self)
    }
    pub fn write(self, value: T) -> Shared<T> {
        let ptr = self.0.0;
        // SAFETY: Slot exposes no references or clones, so this allocation has
        // exactly one owner. Inner is repr(C) and MaybeUninit<T> has T's layout.
        // Initialize before changing its type and forget the uninitialized owner.
        unsafe {
            (*ptr.as_ptr()).value.write(value);
        }
        std::mem::forget(self);
        Shared(ptr.cast())
    }
}

/// Parking lock: std's Linux mutex uses an inline futex, with no lazy heap
/// allocation. Publication may hold this across filesystem I/O, so contention
/// must park rather than repeatedly yield for the duration of a filesystem stall.
/// Recover poisoning because callers already audit panics at job boundaries.
#[cfg(not(target_os = "macos"))]
pub struct Lock<T>(std::sync::Mutex<T>);
#[cfg(not(target_os = "macos"))]
impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    pub fn into_inner(self) -> T {
        self.0
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<T: Default> Default for Lock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// Darwin's std mutex boxes a pthread mutex on first use. Use its native
// allocation-free parking lock directly, so post-head locks remain fallible
// solely through their containing Shared allocation on this platform too.
#[cfg(target_os = "macos")]
#[repr(C)]
struct Unfair(std::cell::UnsafeCell<libc::os_unfair_lock>);
#[cfg(target_os = "macos")]
pub struct Lock<T> {
    raw: Unfair,
    value: std::cell::UnsafeCell<T>,
}
#[cfg(target_os = "macos")]
// SAFETY: the native parking lock serializes every accessor to T.
unsafe impl<T: Send> Send for Lock<T> {}
#[cfg(target_os = "macos")]
// SAFETY: exclusive guarded access transfers T only when T: Send.
unsafe impl<T: Send> Sync for Lock<T> {}
#[cfg(target_os = "macos")]
impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            // SAFETY: os_unfair_lock is an opaque u32; all-zero is Darwin's
            // OS_UNFAIR_LOCK_INIT and a valid initial unlocked value.
            raw: Unfair(std::cell::UnsafeCell::new(unsafe { std::mem::zeroed() })),
            value: std::cell::UnsafeCell::new(value),
        }
    }
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
    pub fn lock(&self) -> Guard<'_, T> {
        // SAFETY: raw is an initialized os_unfair_lock, kept at a stable
        // address by this borrow through the guard's lifetime. Never recursive.
        unsafe {
            libc::os_unfair_lock_lock(self.raw.0.get());
        }
        Guard(self, std::marker::PhantomData)
    }
}
#[cfg(target_os = "macos")]
pub struct Guard<'a, T>(
    &'a Lock<T>,
    std::marker::PhantomData<std::sync::MutexGuard<'a, T>>,
);
#[cfg(target_os = "macos")]
impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock until Drop.
        unsafe { &*self.0.value.get() }
    }
}
#[cfg(target_os = "macos")]
impl<T> std::ops::DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: mutable access is exclusive while this guard holds the lock.
        unsafe { &mut *self.0.value.get() }
    }
}
#[cfg(target_os = "macos")]
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: this guard is non-Send and unlocks on the locking thread.
        unsafe {
            libc::os_unfair_lock_unlock(self.0.raw.0.get());
        }
    }
}
