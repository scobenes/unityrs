//! Handlers reach only process-lived storage and lock-free atomics. Returning
//! handlers restore errno. No Arc, TLS init, formatting, channel or destructor.
use crate::{
    lifetime::{Process, Stop},
    reason::StopReason,
};
use std::{
    io,
    sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering::SeqCst},
};
static PROCESS: AtomicPtr<Process> = AtomicPtr::new(std::ptr::null_mut());
static INIT: AtomicBool = AtomicBool::new(false);
/// The parent-death signal's number. libc's `SIGRTMIN()` is a function call
/// that is not on the async-signal-safe list, so it runs once, here, before
/// any handler is installed (install calls this first); the handler only
/// loads the stored number. Zero means not yet computed.
static PARENT_SIGNAL: AtomicI32 = AtomicI32::new(0);
#[must_use]
pub fn parent_signal() -> i32 {
    let stored = PARENT_SIGNAL.load(SeqCst);
    if stored != 0 {
        return stored;
    }
    #[cfg(target_os = "linux")]
    let n = libc::SIGRTMIN();
    #[cfg(not(target_os = "linux"))]
    let n = libc::SIGUSR1;
    PARENT_SIGNAL.store(n, SeqCst);
    n
}
fn set() -> libc::sigset_t {
    // SAFETY: sigemptyset initializes a sigset_t; only valid signals are added.
    unsafe {
        let mut set = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        for n in [
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGHUP,
            parent_signal(),
            libc::SIGABRT,
        ] {
            libc::sigaddset(&raw mut set, n);
        }
        set
    }
}
/// Call before validation and before any service thread. Inherited pending
/// signals remain held until install explicitly unblocks all five.
pub fn block() -> io::Result<()> {
    let set = set();
    // SAFETY: initialized signal set, null old-mask output.
    mask_result(unsafe {
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut())
    })
}
fn mask_result(rc: i32) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc))
    }
}
pub fn install(process: &'static Process) -> io::Result<()> {
    install_inner(process, false)
}
#[cfg(feature = "test-hooks")]
pub fn install_as_init(process: &'static Process) -> io::Result<()> {
    install_inner(process, true)
}
fn install_inner(process: &'static Process, force_init: bool) -> io::Result<()> {
    PROCESS
        .compare_exchange(
            std::ptr::null_mut(),
            std::ptr::from_ref(process).cast_mut(),
            SeqCst,
            SeqCst,
        )
        .map_err(|_| io::ErrorKind::AlreadyExists)?;
    // SAFETY: getpid takes no arguments or memory.
    INIT.store(unsafe { libc::getpid() } == 1 || force_init, SeqCst);
    // Stored before any handler can run; the handler never calls SIGRTMIN().
    let parent = parent_signal();
    for n in [
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGHUP,
        parent,
        libc::SIGABRT,
    ] {
        // SAFETY: sigaction structs contain integer/pointer fields; zeroed
        // storage is initialized, and sigaction fills the old action on success.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: valid signal and writable old-action output.
        super::cvt(unsafe { libc::sigaction(n, std::ptr::null(), &raw mut old) })?;
        if [libc::SIGINT, libc::SIGHUP].contains(&n) && old.sa_sigaction == libc::SIG_IGN {
            continue;
        }
        let mut action = old;
        action.sa_mask = set();
        action.sa_sigaction = handler as *const () as usize;
        // No SA_RESETHAND, SIGABRT's included: a second SIGABRT on another
        // thread must also meet the handler (and its resets) rather than a
        // default action reset by the first. The handler restores the default
        // itself, after its resets, before raising again.
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: handler has the required C signature and accesses only the
        // fully initialized process-lived value published before installation.
        super::cvt(unsafe { libc::sigaction(n, &raw const action, std::ptr::null_mut()) })?;
    }
    let set = set();
    // SAFETY: initialized set; explicitly removes inherited handled blocks.
    mask_result(unsafe {
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut())
    })
}
extern "C" fn handler(n: i32) {
    // SAFETY: errno points to this thread's C errno storage.
    let saved = unsafe { *super::errno() };
    let pointer = PROCESS.load(SeqCst);
    // SAFETY: only install publishes this pointer, which is process-lived and
    // initialized before any handler is installed. It is never replaced/freed.
    if let Some(process) = unsafe { pointer.as_ref() } {
        if n == libc::SIGABRT {
            process.sockets.reset();
            if INIT.load(SeqCst) {
                // SAFETY: init must use explicit exit status for SIGABRT.
                unsafe { libc::_exit(134) }
            }
            default_abort();
        } else if n == PARENT_SIGNAL.load(SeqCst) {
            process.request(Stop::external(StopReason::ParentDied));
        } else {
            let reason = match n {
                libc::SIGTERM => StopReason::Sigterm,
                libc::SIGINT => StopReason::Sigint,
                _ => StopReason::Sighup,
            };
            process.signal(reason);
        }
    }
    // SAFETY: same live thread-local errno location saved on entry.
    unsafe {
        *super::errno() = saved;
    }
}
/// After the resets: SIGABRT's default action, then raise. Both calls are
/// async-signal-safe. SIGABRT stays blocked in this handler, so the raised
/// signal is delivered, with its default action, when the handler returns.
fn default_abort() {
    // SAFETY: zeroed sigaction is a valid all-integer/pointer value; SIG_DFL
    // installs no callback, and an empty mask and zero flags are valid.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: initialized action, null old-action output.
    unsafe {
        libc::sigaction(libc::SIGABRT, &raw const action, std::ptr::null_mut());
        libc::raise(libc::SIGABRT);
    }
}
/// Linux watches the spawning THREAD, so harnesses spawn/reap on one long-lived
/// thread. Compare the supplied pid only after arming PDEATHSIG.
pub fn parent(process: &Process, expected: Option<u32>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: scalar prctl operation; no memory pointers.
        super::cvt(unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, parent_signal()) })?;
    }
    // SAFETY: getppid takes no arguments or borrowed memory.
    let pid = u32::try_from(unsafe { libc::getppid() }).map_err(|_| io::ErrorKind::InvalidData)?;
    if expected.is_some_and(|e| e != pid) {
        process.request(Stop::external(StopReason::ParentDied));
    }
    Ok(())
}
