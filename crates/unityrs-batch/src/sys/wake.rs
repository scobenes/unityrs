use std::io;
use std::{os::fd::AsRawFd, sync::Arc};

impl AsRawFd for Wake {
    fn as_raw_fd(&self) -> i32 {
        self.read.as_raw_fd()
    }
}
struct WakeOwner(Arc<Wake>);
impl AsRawFd for WakeOwner {
    fn as_raw_fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
}
/// Exactly one registration per wake on its intended runtime. Owning the Arc
/// inside `AsyncFd` keeps its open description unchanged until deregistration.
pub struct RegisteredWake(tokio::io::unix::AsyncFd<WakeOwner>);
impl RegisteredWake {
    /// Registers on `handle`'s reactor (under `Handle::enter`), so a caller
    /// that holds only a runtime handle can register too.
    pub fn new(wake: &Arc<Wake>, handle: &tokio::runtime::Handle) -> io::Result<Self> {
        let _entered = handle.enter();
        // SAFETY: WakeOwner retains the descriptor for the registration's
        // lifetime; Wake never closes/replaces it and is outside socket reset.
        unsafe { tokio::io::unix::AsyncFd::register(WakeOwner(wake.clone())) }
            .map(Self)
            .map_err(|e| e.into_parts().1)
    }
    /// Finisher waits register this future BEFORE loading `hard()`. Once armed,
    /// use the deadline; this permanently ready wake must never be awaited again.
    pub async fn ready(&self) -> io::Result<()> {
        let _ = self.0.readable().await?;
        Ok(())
    }
    /// A repeatable wait, for a loop (the controller's stop and end wakes):
    /// wait for readiness, drain the descriptor, then clear the readiness.
    /// A notify after the drain is not lost: it is a new reactor event, whose
    /// newer tick `clear_ready` leaves set, so the next wait returns at once.
    pub async fn wait(&self) -> io::Result<()> {
        let mut guard = self.0.readable().await?;
        self.0.get_ref().0.drain()?;
        guard.clear_ready();
        Ok(())
    }
}
/// Nonblocking wake descriptor. Linux uses `eventfd`; the portable test host
/// uses a nonblocking socket pair with the same coalescing wake semantics.
pub struct Wake {
    read: std::os::fd::OwnedFd,
    write: std::os::fd::OwnedFd,
}
impl Wake {
    pub fn new() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::{AsFd, FromRawFd};
            // SAFETY: eventfd takes only scalar arguments and returns a new fd.
            let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the successful call transferred one newly owned fd.
            let read = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            let write = read.as_fd().try_clone_to_owned()?;
            Ok(Self { read, write })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let (read, write) = std::os::unix::net::UnixStream::pair()?;
            read.set_nonblocking(true)?;
            write.set_nonblocking(true)?;
            Ok(Self {
                read: read.into(),
                write: write.into(),
            })
        }
    }
    /// A full nonblocking wake already means the reader must run. Any
    /// positive count is a wake: Linux's eventfd takes all 8 bytes or none,
    /// and a partial write to the macOS test host's socket pair still leaves
    /// bytes the reader drains (`errno` is not read after a success).
    pub fn notify(&self) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let word = 1_u64;
        loop {
            // SAFETY: word is readable for its complete size; fd is live.
            let n = unsafe {
                libc::write(
                    self.write.as_raw_fd(),
                    std::ptr::from_ref(&word).cast(),
                    size_of::<u64>(),
                )
            };
            if n > 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock => return Ok(()),
                io::ErrorKind::Interrupted => {}
                _ => return Err(e),
            }
        }
    }
    /// Read before draining the end channel. `EAGAIN` is an empty wake.
    pub fn drain(&self) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        loop {
            let mut word = 0_u64;
            // SAFETY: word is writable for its size and the descriptor is live.
            let n = unsafe {
                libc::read(
                    self.read.as_raw_fd(),
                    std::ptr::from_mut(&mut word).cast(),
                    size_of::<u64>(),
                )
            };
            if n > 0 {
                continue;
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock => return Ok(()),
                io::ErrorKind::Interrupted => {}
                _ => return Err(e),
            }
        }
    }
}
/// W1/W2: one bounded wait, including interruptions. The caller always drains
/// the end wake and channel after return, even on a wake-only tick.
pub fn poll_wakes(end: &Wake, wake: &Wake) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut fds = [
        libc::pollfd {
            fd: end.read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: fds contains two initialized pollfd values for this call.
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1_000) };
    if rc < 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    if fds
        .iter()
        .any(|f| f.revents & (libc::POLLERR | libc::POLLNVAL | libc::POLLHUP) != 0)
    {
        return Err(io::Error::other("wake descriptor closed"));
    }
    Ok(())
}
