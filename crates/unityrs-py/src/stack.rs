//! How much of the calling thread's own stack is left below this frame, so
//! work whose allowance it covers can run inline instead of on a worker.
//! Anything unsure answers no: the work then runs on a worker of known size.

/// Stack kept free beyond a call's allowance: the guard area some platforms
/// count in a thread's stack, the binding's own frames, and slack for frames
/// larger than the allowance's estimate.
const MARGIN: usize = 1 << 20;

/// Whether this thread has `allowance` of stack left (plus the margin).
pub(crate) fn covers(allowance: usize) -> bool {
    // This thread's stack, once asked: `(low, high)`, `(0, 0)` unknown.
    // A pthread's bounds are fixed for its life; the main thread's limit
    // is read again below, on every call.
    thread_local! {
        static BOUNDS: std::cell::Cell<Option<(usize, usize)>> = const { std::cell::Cell::new(None) };
    }
    let (low, high) = BOUNDS.with(|cell| {
        cell.get().unwrap_or_else(|| {
            let bounds = bounds().unwrap_or((0, 0));
            cell.set(Some(bounds));
            bounds
        })
    });
    let marker = 0_u8;
    let here = std::hint::black_box(std::ptr::from_ref(&marker)) as usize;
    match main_limit() {
        Limit::Fixed => fits(here, low, high, None, allowance),
        Limit::Grows(limit) => fits(here, low, high, Some(limit), allowance),
        Limit::Unknown => false,
    }
}

/// Whether a frame at `here` has `allowance` plus the margin left in the
/// stack `low..high`, of which at most `limit` below `high` may be used
/// (the main thread's `RLIMIT_STACK`). A frame outside the stack (one
/// switched to another stack) has unknown room.
fn fits(here: usize, low: usize, high: usize, limit: Option<usize>, allowance: usize) -> bool {
    let low = limit.map_or(low, |limit| low.max(high.saturating_sub(limit)));
    low != 0 && low < here && here < high && here - low >= allowance.saturating_add(MARGIN)
}

/// How far this thread's stack may grow below its top.
enum Limit {
    /// A thread made with a fixed stack, or an unlimited main thread.
    Fixed,
    /// The main thread, whose stack grows on demand up to the current
    /// `RLIMIT_STACK`, which can be lowered after its bounds are read.
    Grows(usize),
    Unknown,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main_limit() -> Limit {
    #[cfg(target_os = "linux")]
    // SAFETY: neither call has preconditions. The raw syscall avoids
    // glibc's newer gettid wrapper.
    let main = unsafe { libc::syscall(libc::SYS_gettid) == libc::c_long::from(libc::getpid()) };
    #[cfg(target_os = "macos")]
    // SAFETY: no preconditions.
    let main = unsafe { libc::pthread_main_np() } == 1;
    if !main {
        return Limit::Fixed;
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes the struct it is given.
    if unsafe { libc::getrlimit(libc::RLIMIT_STACK, &raw mut limit) } != 0 {
        return Limit::Unknown;
    }
    if limit.rlim_cur == libc::RLIM_INFINITY {
        return Limit::Fixed;
    }
    Limit::Grows(usize::try_from(limit.rlim_cur).unwrap_or(usize::MAX))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main_limit() -> Limit {
    Limit::Unknown
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
        assert!(fits(107 * MIB, low, high, None, 4 * MIB));
        assert!(!fits(low + 4 * MIB + MARGIN - 1, low, high, None, 4 * MIB));
        // A frame on a stack switched to elsewhere, above or below.
        assert!(!fits(200 * MIB, low, high, None, MIB));
        assert!(!fits(50 * MIB, low, high, None, MIB));
        // Unknown bounds.
        assert!(!fits(107 * MIB, 0, 0, None, MIB));
    }

    #[test]
    fn a_lowered_stack_limit_shrinks_the_main_thread_stack() {
        let (low, high) = (100 * MIB, 108 * MIB);
        assert!(fits(107 * MIB, low, high, Some(8 * MIB), 4 * MIB));
        assert!(!fits(107 * MIB, low, high, Some(2 * MIB), 4 * MIB));
        // A raised limit never extends the stack past its known bounds.
        assert!(!fits(107 * MIB, low, high, Some(64 * MIB), 7 * MIB));
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
