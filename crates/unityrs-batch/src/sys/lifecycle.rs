//! Raw lifetime descriptors. All ownership is established before handlers or
//! lifecycle polls become reachable. No caller description's flags are changed.
use crate::lifetime::Boot;
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

pub struct Pipe {
    read: OwnedFd,
    write: OwnedFd,
}
impl Pipe {
    /// Linux creates both ends close-on-exec and nonblocking atomically
    /// (`pipe2`), so a fork on another thread can never inherit them. macOS,
    /// a test host only, has no `pipe2` and sets the flags just after.
    pub fn new() -> io::Result<Self> {
        let mut fds = [-1; 2];
        #[cfg(target_os = "linux")]
        let rc = {
            // SAFETY: two writable integers; success transfers two new fds.
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }
        };
        #[cfg(not(target_os = "linux"))]
        let rc = {
            // SAFETY: two writable integers; success transfers two new fds.
            unsafe { libc::pipe(fds.as_mut_ptr()) }
        };
        super::cvt(rc)?;
        let [r, w] = fds;
        // SAFETY: the pipe call succeeded, each fd is transferred exactly once.
        let pipe = unsafe {
            Self {
                read: OwnedFd::from_raw_fd(r),
                write: OwnedFd::from_raw_fd(w),
            }
        };
        #[cfg(not(target_os = "linux"))]
        for fd in [r, w] {
            // SAFETY: both fds are newly owned pipe descriptions, not caller fds.
            super::cvt(unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) })?;
            // SAFETY: FD_CLOEXEC changes only this new descriptor's flags.
            super::cvt(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) })?;
        }
        Ok(pipe)
    }
    pub fn notify(&self) {
        let byte = 1_u8;
        loop {
            // SAFETY: readable byte and process-lived nonblocking pipe.
            let n = unsafe { libc::write(self.write.as_raw_fd(), (&raw const byte).cast(), 1) };
            if n >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return;
            }
        }
    }
    #[must_use]
    pub fn drain(&self) -> bool {
        let mut bytes = [0_u8; 256];
        let mut read = false;
        loop {
            // SAFETY: initialized writable buffer and live pipe reader.
            let n = unsafe {
                libc::read(
                    self.read.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if n > 0 {
                read = true;
                continue;
            }
            if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return read;
        }
    }
}
impl AsRawFd for Pipe {
    fn as_raw_fd(&self) -> i32 {
        self.read.as_raw_fd()
    }
}
pub struct Timer {
    fd: OwnedFd,
    #[cfg(feature = "test-hooks")]
    fail_arm: bool,
}
impl Timer {
    pub fn new() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: scalar arguments, success transfers one new descriptor.
            let fd = unsafe {
                libc::timerfd_create(libc::CLOCK_BOOTTIME, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC)
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: timerfd_create returned a newly owned fd.
            Ok(Self {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
                #[cfg(feature = "test-hooks")]
                fail_arm: false,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(io::Error::from_raw_os_error(libc::ENOSYS))
        }
    }
    pub fn arm(&self, deadline: Boot) -> io::Result<()> {
        #[cfg(feature = "test-hooks")]
        if self.fail_arm {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        #[cfg(target_os = "linux")]
        {
            // Zero disarms timerfd, so an already due instant uses 1 ns.
            let value = libc::itimerspec {
                it_interval: libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                it_value: libc::timespec {
                    tv_sec: libc::time_t::try_from(deadline.0 / 1000)
                        .map_err(|_| io::ErrorKind::InvalidInput)?,
                    tv_nsec: i64::try_from(deadline.0 % 1000 * 1_000_000)
                        .map_err(|_| io::ErrorKind::InvalidInput)?
                        .max(i64::from(deadline.0 == 0)),
                },
            };
            // SAFETY: live timer and complete initialized itimerspec input.
            super::cvt(unsafe {
                libc::timerfd_settime(
                    self.as_raw_fd(),
                    libc::TFD_TIMER_ABSTIME,
                    &raw const value,
                    std::ptr::null_mut(),
                )
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = deadline;
            Err(io::Error::from_raw_os_error(libc::ENOSYS))
        }
    }
    pub fn drain(&self) {
        let mut n = 0_u64;
        // SAFETY: writable counter storage; nonblocking descriptor we own.
        unsafe {
            libc::read(self.as_raw_fd(), (&raw mut n).cast(), 8);
        }
    }
}
#[cfg(feature = "test-hooks")]
pub fn test_mark() {
    let bytes = b"marked\n";
    // SAFETY: isolated counted children supply a drained pipe stdout; one
    // fixed write publishes their mark without allocator/formatter/TLS work.
    unsafe {
        libc::write(1, bytes.as_ptr().cast(), bytes.len());
    }
}
impl AsRawFd for Timer {
    fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}
#[cfg(feature = "test-hooks")]
impl Timer {
    pub fn fail_arm(&mut self) {
        self.fail_arm = true;
    }
}
/// One poll only: caller recomputes BOOTTIME after every return, EINTR included.
pub fn poll(fds: &mut [libc::pollfd], timeout_ms: i32) -> io::Result<()> {
    let count = libc::nfds_t::try_from(fds.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: writable slice describes exactly the provided number of pollfds.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout_ms) };
    if n >= 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::Interrupted {
        Ok(())
    } else {
        Err(e)
    }
}
#[derive(Clone, Copy)]
pub struct Stdin {
    fd: i32,
    socket: bool,
    terminal: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Again,
    Eof,
    Ended,
}
impl Stdin {
    pub fn prepare() -> io::Result<Self> {
        Self::at(0)
    }
    fn at(fd: i32) -> io::Result<Self> {
        let mut s = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: writable complete stat; fstat borrows fd 0.
        super::cvt(unsafe { libc::fstat(fd, s.as_mut_ptr()) })?;
        // SAFETY: successful fstat initialized the structure.
        let kind = unsafe { s.assume_init() }.st_mode & libc::S_IFMT;
        // SAFETY: scalar descriptor queries, no ownership or flag changes.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        // SAFETY: scalar descriptor query, no caller memory.
        let terminal = unsafe { libc::isatty(fd) } == 1;
        if flags < 0
            || flags & libc::O_ACCMODE == libc::O_WRONLY
            || !(kind == libc::S_IFIFO || kind == libc::S_IFSOCK || terminal)
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self {
            fd,
            socket: kind == libc::S_IFSOCK,
            terminal,
        })
    }
    #[must_use]
    pub fn fd(self) -> i32 {
        self.fd
    }
    pub fn read(self, events: i16, buffer: &mut [u8; 4096]) -> Input {
        let mut count = 0_i32;
        // SAFETY: writable integer for FIONREAD; no description flags change.
        let known = unsafe { libc::ioctl(self.fd, libc::FIONREAD, &raw mut count) } == 0;
        if events & libc::POLLHUP != 0 && (!known || count <= 0) {
            return Input::Eof;
        }
        let n = usize::try_from(count).unwrap_or(0).clamp(1, buffer.len());
        // SAFETY: caller's buffer is writable for n bytes. Socket recv cannot
        // block; a pipe/terminal read follows readiness and FIONREAD, and can
        // block only when another reader takes the bytes first, which holds
        // only the stdin watch's own thread (lifetime/stdin.rs).
        let n = unsafe {
            if self.socket {
                libc::recv(self.fd, buffer.as_mut_ptr().cast(), n, libc::MSG_DONTWAIT)
            } else {
                libc::read(self.fd, buffer.as_mut_ptr().cast(), n)
            }
        };
        if n == 0 {
            return Input::Eof;
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if matches!(
                e.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                return Input::Again;
            }
            let raw = e.raw_os_error();
            if (self.terminal && raw == Some(libc::EIO))
                || (self.socket
                    && matches!(
                        raw,
                        Some(libc::ECONNRESET | libc::ETIMEDOUT | libc::ENOTCONN)
                    ))
            {
                return Input::Eof;
            }
            return Input::Ended;
        }
        if events & (libc::POLLERR | libc::POLLNVAL) != 0 {
            Input::Ended
        } else {
            Input::Again
        }
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
/// No unwinding, destructors, flushing or writes precede the process exit.
pub fn exit(code: crate::reason::Code, sockets: &super::reset::SocketTable) -> ! {
    sockets.reset();
    // SAFETY: _exit takes only a status; all socket resets completed above.
    unsafe { libc::_exit(i32::from(code.value())) }
}
#[cfg(feature = "test-hooks")]
pub(crate) fn trace(file: &std::fs::File, line: &[u8]) {
    // SAFETY: startup validated this owned nonblocking FIFO; line is fixed,
    // readable and smaller than PIPE_BUF. No retry or lock can delay hard exit.
    unsafe {
        libc::write(file.as_raw_fd(), line.as_ptr().cast(), line.len());
    }
}
