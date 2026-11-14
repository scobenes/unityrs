//! Isolated process probes only; production binaries contain none of these.
use std::io;
pub fn raise_abort() {
    // SAFETY: scalar signal operation against this process only.
    unsafe {
        libc::raise(libc::SIGABRT);
    }
}
pub fn ignore(signal: i32) -> io::Result<()> {
    // SAFETY: SIG_IGN installs no Rust callback; valid test signal is scalar.
    if unsafe { libc::signal(signal, libc::SIG_IGN) } == libc::SIG_ERR {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn raise(signal: i32) -> io::Result<()> {
    // SAFETY: raises a test-selected signal against the calling thread only.
    super::cvt(unsafe { libc::raise(signal) })
}
#[cfg(target_os = "linux")]
pub fn subreaper() -> io::Result<()> {
    // SAFETY: isolated scene supervisor only; scalar prctl operation.
    super::cvt(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) })
}
pub fn flags(fd: i32) -> io::Result<i32> {
    // SAFETY: scalar read-only descriptor query, no ownership or flag changes.
    let n = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n)
    }
}
