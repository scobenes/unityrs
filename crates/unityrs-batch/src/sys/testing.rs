//! Isolated process probes only; production binaries contain none of these.
use std::io;
use std::{os::unix::net::UnixStream, sync::Arc, time::Duration};

#[cfg(all(test, target_os = "linux"))]
#[path = "testing_tests.rs"]
mod tests;

/// Shutting down on drop releases a child even if the replying thread panics.
/// The callback retains the same parent fd so it cannot be reused before fork.
pub struct Rendezvous(Arc<UnixStream>);
impl Rendezvous {
    #[must_use]
    pub fn stream(&self) -> &UnixStream {
        &self.0
    }
}
impl Drop for Rendezvous {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// Force fork/exec and rendezvous with the child before exec closes its fds.
/// The caller supplies the testkit's deadlock guard and replies with one byte
/// before waiting for the command. Dropping the guard cancels the rendezvous.
pub fn hold_before_exec(
    cmd: &mut std::process::Command,
    timeout: Duration,
) -> io::Result<Rendezvous> {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let (parent, child) = UnixStream::pair()?;
    parent.set_read_timeout(Some(timeout))?;
    child.set_read_timeout(Some(timeout))?;
    let parent = Arc::new(parent);
    let inherited_parent = Arc::clone(&parent);
    // SAFETY: the callback uses only async-signal-safe close/send/recv and
    // errno reads. It neither allocates nor locks; both captured fds stay live
    // until fork. The closed parent fd is never used again in the child;
    // exec or the spawn error path's _exit runs without Rust destructors.
    unsafe {
        cmd.pre_exec(move || {
            // Otherwise our own inherited peer prevents EOF on parent death.
            // Do not retry close: EINTR may already have closed the descriptor.
            if libc::close(inherited_parent.as_raw_fd()) == -1 {
                return Err(io::Error::last_os_error());
            }
            let mut byte = 1_u8;
            // Propagate EINTR on both calls: retrying a timed socket operation
            // restarts its timeout, so repeated signals could hold spawn forever.
            let n = libc::send(
                child.as_raw_fd(),
                (&raw const byte).cast(),
                1,
                libc::MSG_NOSIGNAL,
            );
            if n == -1 {
                return Err(io::Error::last_os_error());
            }
            if n != 1 {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            match libc::recv(child.as_raw_fd(), (&raw mut byte).cast(), 1, 0) {
                1 => Ok(()),
                0 => Err(io::Error::from_raw_os_error(libc::ECANCELED)),
                _ => Err(io::Error::last_os_error()),
            }
        });
    }
    Ok(Rendezvous(parent))
}
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
