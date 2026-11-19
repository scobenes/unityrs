//! How much of the calling thread's own stack is left below this frame, so
//! work whose allowance it covers can run inline instead of on a worker.
//! Anything unsure answers no: the work then runs on a worker of known size.

/// Stack kept free beyond a call's allowance: the guard area some platforms
/// count in a thread's stack, the binding's own frames, and slack for frames
/// larger than the allowance's estimate.
const MARGIN: usize = 1 << 20;

/// Whether this thread has `allowance` of stack left (plus the margin). The
/// main thread never answers yes: its stack grows on demand, and how far
/// depends on `RLIMIT_STACK` and on what is later mapped below it. Other
/// threads' stacks are fixed; their bounds are read on every call.
pub(crate) fn covers(allowance: usize) -> bool {
    if !fixed() {
        return false;
    }
    let Some((low, high)) = bounds() else {
        return false;
    };
    let marker = 0_u8;
    let here = std::hint::black_box(std::ptr::from_ref(&marker)) as usize;
    fits(here, low, high, allowance)
}

/// Whether a frame at `here` has `allowance` plus the margin left in the
/// stack `low..high`. A frame outside it (on a stack switched to) has
/// unknown room.
fn fits(here: usize, low: usize, high: usize, allowance: usize) -> bool {
    low != 0 && low < here && here < high && here - low >= allowance.saturating_add(MARGIN)
}

/// Whether this thread's stack is fixed: any thread but the process's main.
#[cfg(target_os = "linux")]
fn fixed() -> bool {
    // SAFETY: neither call has preconditions. The raw syscall avoids
    // glibc's newer gettid wrapper.
    unsafe { libc::syscall(libc::SYS_gettid) != libc::c_long::from(libc::getpid()) }
}

#[cfg(target_os = "macos")]
fn fixed() -> bool {
    // SAFETY: no preconditions.
    unsafe { libc::pthread_main_np() == 0 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn fixed() -> bool {
    false
}

/// This thread's stack, lowest and highest address; stacks grow down on
/// every platform this answers for.
#[cfg(target_os = "linux")]
fn bounds() -> Option<(usize, usize)> {
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
        let low = address as usize;
        (found == 0 && size > 0).then_some((low, low.checked_add(size)?))
    }
}

#[cfg(target_os = "macos")]
fn bounds() -> Option<(usize, usize)> {
    // SAFETY: both query the calling thread, which is alive.
    let (high, size) = unsafe {
        let thread = libc::pthread_self();
        (
            libc::pthread_get_stackaddr_np(thread) as usize,
            libc::pthread_get_stacksize_np(thread),
        )
    };
    (size > 0).then_some((high.checked_sub(size)?, high))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn bounds() -> Option<(usize, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::{MARGIN, fits};

    const MIB: usize = 1 << 20;

    #[test]
    fn room_is_measured_inside_the_stack_only() {
        let (low, high) = (100 * MIB, 108 * MIB);
        assert!(fits(107 * MIB, low, high, 4 * MIB));
        assert!(!fits(low + 4 * MIB + MARGIN - 1, low, high, 4 * MIB));
        // A frame on a stack switched to elsewhere, above or below.
        assert!(!fits(200 * MIB, low, high, MIB));
        assert!(!fits(50 * MIB, low, high, MIB));
        // Unknown bounds.
        assert!(!fits(107 * MIB, 0, 0, MIB));
    }

    #[test]
    fn small_thread_stacks_do_not_cover_large_allowances() {
        let answer = |stack| {
            std::thread::Builder::new()
                .stack_size(stack)
                .spawn(|| super::covers(MIB))
                .is_ok_and(|thread| thread.join().unwrap_or(true))
        };
        assert!(!answer(256 << 10));
        assert_eq!(
            answer(64 * MIB),
            cfg!(any(target_os = "linux", target_os = "macos"))
        );
    }
}
