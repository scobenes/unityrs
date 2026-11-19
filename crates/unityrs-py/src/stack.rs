//! How much of the calling thread's own stack is left below this frame, so
//! work whose allowance it covers can run inline instead of on a worker.

/// Stack kept free beyond a call's allowance: the guard area some platforms
/// count in a thread's stack, and the binding's own frames.
const MARGIN: usize = 256 << 10;

/// Whether this thread has `allowance` of stack left (plus the margin).
/// Unknown bounds answer no: the work then runs on a worker of known size.
pub(crate) fn covers(allowance: usize) -> bool {
    // The lowest address, once known; 0 unknown, `usize::MAX` not yet asked.
    thread_local! {
        static LOW: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    }
    let low = LOW.with(|cell| {
        if cell.get() == usize::MAX {
            cell.set(low_bound().unwrap_or(0));
        }
        cell.get()
    });
    let marker = 0_u8;
    let here = std::hint::black_box(std::ptr::from_ref(&marker)) as usize;
    low != 0
        && here
            .checked_sub(low)
            .is_some_and(|room| room >= allowance.saturating_add(MARGIN))
}

/// The lowest address of this thread's stack; stacks grow down on every
/// platform this answers for.
#[cfg(target_os = "linux")]
fn low_bound() -> Option<usize> {
    // SAFETY: the attribute object is initialised by pthread_getattr_np
    // before it is read, and destroyed exactly once after a success.
    unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &raw mut attr) != 0 {
            return None;
        }
        let mut address = std::ptr::null_mut();
        let mut size = 0;
        let found = libc::pthread_attr_getstack(&raw const attr, &raw mut address, &raw mut size);
        libc::pthread_attr_destroy(&raw mut attr);
        (found == 0 && size > 0).then_some(address as usize)
    }
}

#[cfg(target_os = "macos")]
fn low_bound() -> Option<usize> {
    // SAFETY: both query the calling thread, which is alive.
    let (top, size) = unsafe {
        let thread = libc::pthread_self();
        (
            libc::pthread_get_stackaddr_np(thread) as usize,
            libc::pthread_get_stacksize_np(thread),
        )
    };
    top.checked_sub(size).filter(|_| size > 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn low_bound() -> Option<usize> {
    None
}
