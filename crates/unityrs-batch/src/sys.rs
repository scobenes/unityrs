//! The engine's C library calls, including spool operations whose Unix paths
//! must allocate fallibly. Each call has a `// SAFETY:` comment; with
//! [`crate::alloc`], the crate's only `unsafe` code (`tests/unsafe_scan.rs`
//! pins both).
//!
//! - [`ignore_job_control`]: `docs/plans/batch.md` 2.11 step 1's `SIG_IGN`
//!   for SIGTTIN, SIGTTOU and SIGXFSZ, so the engine's own reads and
//!   writes of a terminal never stop the process (a background process
//!   group under `tostop`), and a write past `RLIMIT_FSIZE` fails with
//!   `EFBIG` instead of killing it.
//! - [`boottime`]: `CLOCK_BOOTTIME`, which the progress stream's `ms`
//!   counts in (`docs/BATCH_SPEC.md` section 10), suspend included.
//! - [`clock_ticks`]: `sysconf(_SC_CLK_TCK)`, the unit of
//!   `/proc/self/stat`'s start time.

use std::io;
use std::time::Duration;

// Match std's large-file ABI on 32-bit Linux as well as 64-bit hosts.
#[cfg(not(all(target_os = "linux", any(target_env = "gnu", target_env = "uclibc"))))]
use libc::{lstat as spool_lstat, open as spool_open, stat as SpoolStat};
#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "uclibc")))]
use libc::{lstat64 as spool_lstat, open64 as spool_open, stat64 as SpoolStat};

/// Clear only `O_NONBLOCK` on an opened regular local source, after `fstat`.
pub fn blocking_source(file: &std::fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: file owns a live descriptor; F_GETFL uses no variadic argument.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL accepts this integer flag mask, preserving all flags
    // except O_NONBLOCK. The descriptor remains owned throughout the call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The signals 2.11 step 1 sets to `SIG_IGN`.
pub const IGNORED: [(libc::c_int, &str); 3] = [
    (libc::SIGTTIN, "SIGTTIN"),
    (libc::SIGTTOU, "SIGTTOU"),
    (libc::SIGXFSZ, "SIGXFSZ"),
];

/// Sets each of [`IGNORED`] to `SIG_IGN`.
///
/// # Errors
/// `signal`'s error, for the first installation the host refuses (for
/// example, under a syscall filter). Each signal itself may be ignored.
/// On failure the caller exits 69 and suppresses output on any stream
/// whose `fstat` reports a character device: SIGTTOU may still stop a
/// terminal write, and a denied ioctl can make terminal detection fail.
/// Pipes, FIFOs, sockets and regular files keep the early writer's shared
/// budget and its regular-file writeback exception.
pub fn ignore_job_control() -> io::Result<()> {
    for (sig, _) in IGNORED {
        // SAFETY: `signal` with `SIG_IGN` installs no handler, so no code
        // of ours runs in a signal context; `sig` is a valid signal
        // number, which can be ignored (not SIGKILL or SIGSTOP).
        let old = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if old == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// `CLOCK_BOOTTIME`'s time since boot; `None` off Linux, or when the call
/// fails.
#[must_use]
pub fn boottime() -> Option<Duration> {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid, writable `timespec` for the call's
        // duration; `CLOCK_BOOTTIME` is a clock id Linux has had since
        // 2.6.39.
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut ts) };
        if rc != 0 {
            return None;
        }
        let secs = u64::try_from(ts.tv_sec).ok()?;
        let nanos = u32::try_from(ts.tv_nsec).ok()?;
        Some(Duration::new(secs, nanos))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// `sysconf(_SC_CLK_TCK)`: clock ticks a second; `None` when unknown.
#[must_use]
pub fn clock_ticks() -> Option<u64> {
    // SAFETY: `sysconf` reads a configuration value and touches no memory
    // of ours.
    let n = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(n).ok().filter(|&n| n > 0)
}

/// A real terminal for the early writer's routing tests, without changing
/// the test process's controlling terminal or signal dispositions.
#[cfg(test)]
pub(crate) fn terminal_pair() -> io::Result<(std::fs::File, std::fs::File)> {
    use std::os::fd::FromRawFd;

    let (mut master, mut slave) = (-1, -1);
    // SAFETY: both output pointers are valid for the call. Null optional
    // pointers request default settings and no terminal-name output.
    let rc = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful openpty returns two newly owned descriptors;
    // each is transferred to exactly one File and closed by its drop.
    Ok(unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    })
}

/// `statvfs` caller-available bytes for the spool reserve check. Equality
/// with the reserve succeeds. Overflow is a host I/O error, never wraparound.
#[allow(clippy::useless_conversion)] // libc field widths vary across Unix targets.
pub fn available_bytes(path: &std::path::Path) -> io::Result<u64> {
    let path = terminated_path(path)?;
    let mut facts = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and facts points to writable storage
    // for one statvfs; both remain alive throughout the call.
    if unsafe { libc::statvfs(path.as_ptr().cast(), facts.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful statvfs initializes the complete output struct.
    let facts = unsafe { facts.assume_init() };
    let blocks = u64::from(facts.f_bavail);
    let unit = u64::from(facts.f_frsize);
    blocks
        .checked_mul(unit)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
}

// std's Unix path adapters can allocate infallibly for long paths. Keep
// these phase-path calls behind one fallible NUL-termination helper.
fn terminated_path(path: &std::path::Path) -> io::Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut terminated = Vec::new();
    terminated
        .try_reserve_exact(
            bytes
                .len()
                .checked_add(1)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?,
        )
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    terminated.extend_from_slice(bytes);
    terminated.push(0);
    Ok(terminated)
}

/// The spool's exclusive create or no-follow read, with fallible path storage.
pub(crate) fn spool_file(path: &std::path::Path, create: bool) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    let path = terminated_path(path)?;
    let flags = libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | if create {
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY
        };
    let fd = retry_interrupted(|| {
        // SAFETY: path is NUL-terminated and alive throughout open; the mode
        // argument is supplied for O_CREAT.
        let fd = unsafe { spool_open(path.as_ptr().cast(), flags, 0o600_u32) };
        if fd == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    })?;
    // SAFETY: open returned a newly owned descriptor, transferred exactly once.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// How many times an interrupted call is retried before its `EINTR` is
/// returned as an I/O error: enough for any ordinary signal, and a bound,
/// so a signal storm cannot hold the phase in one call.
pub(crate) const INTERRUPTED_RETRIES: u32 = 64;

/// Run `call`, retrying it while it fails with `EINTR`, at most
/// [`INTERRUPTED_RETRIES`] times after the first try.
pub(crate) fn retry_interrupted<T>(mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut retries = 0;
    loop {
        match call() {
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted && retries < INTERRUPTED_RETRIES =>
            {
                retries += 1;
            }
            result => return result,
        }
    }
}

/// Check a sealed regular file without following symlinks or adopting claims.
/// On 32-bit GNU targets `lstat64` can refuse an out-of-range timestamp with
/// `EOVERFLOW`, which is then `host_io`; 32-bit hosts are not a supported or
/// tested target, and the engine's own spool files never carry such times.
pub(crate) fn spool_length(path: &std::path::Path) -> io::Result<u64> {
    let path = terminated_path(path)?;
    let mut stat = std::mem::MaybeUninit::<SpoolStat>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable storage for
    // one stat. Both remain alive throughout lstat.
    if unsafe { spool_lstat(path.as_ptr().cast(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful lstat initialized the complete structure.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    u64::try_from(stat.st_size).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

/// Unlink exactly one generated file with fallible path storage.
pub(crate) fn spool_unlink(path: &std::path::Path) -> io::Result<()> {
    let path = terminated_path(path)?;
    // SAFETY: path is NUL-terminated and alive throughout unlink.
    if unsafe { libc::unlink(path.as_ptr().cast()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
    /// A full nonblocking wake already means the reader must run.
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
            if n == 8 {
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

/// Open a trusted prepared root once, before any deletion can start.
pub fn open_directory(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    let path = terminated_path(path)?;
    // SAFETY: path is NUL-terminated and open returns a new owned descriptor.
    let fd = unsafe { spool_open(path.as_ptr().cast(), directory_flags()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful open transferred exactly one new descriptor.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}
fn directory_flags() -> libc::c_int {
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK
}

/// Attempt outputs and records are flat. Allow seven extra directory levels
/// for cleanup, while bounding cursor storage and open descriptors.
pub const REMOVE_DEPTH: usize = 8;

/// Cooperative deletion anchored to the spool's held root. Every relative
/// component is opened with `openat`; even the top removal uses its parent fd.
pub fn remove_tree(
    root: &std::fs::File,
    relative: &std::path::Path,
    stopped: &dyn Fn() -> bool,
) -> Result<(), crate::spool::Error> {
    use crate::spool::Error;
    use std::{ffi::CStr, os::fd::AsRawFd, path::Component};
    if stopped() {
        return Err(Error::Stopped);
    }
    let mut components = relative.components().peekable();
    let mut parent = root.try_clone()?;
    let top = loop {
        let Some(Component::Normal(name)) = components.next() else {
            return Err(Error::Internal);
        };
        let name = terminated_path(std::path::Path::new(name))?;
        if components.peek().is_none() {
            break name;
        }
        parent = directory_at(parent.as_raw_fd(), &name)?;
    };
    let file = directory_at(parent.as_raw_fd(), &top)?;
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(REMOVE_DEPTH)
        .map_err(|_| Error::HostMemory)?;
    stack.push(Cursor::new(file, None)?);
    let mut entries = 0_u64;
    while let Some(cursor) = stack.last() {
        // SAFETY: errno is this thread's writable errno; cursor.dir is live.
        let entry = unsafe {
            *errno() = 0;
            libc::readdir(cursor.dir)
        };
        if entry.is_null() {
            // SAFETY: the thread-local errno pointer remains valid.
            let error = unsafe { *errno() };
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error).into());
            }
            let name = stack.pop().ok_or(Error::Internal)?.name.take();
            if let Some(ancestor) = stack.last() {
                unlink_at(
                    ancestor.fd(),
                    &name.ok_or(Error::Internal)?,
                    libc::AT_REMOVEDIR,
                )?;
            }
            continue;
        }
        // SAFETY: readdir's non-null entry holds a NUL-terminated d_name,
        // valid until the next call. Copy before any subsequent readdir.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if entries.is_multiple_of(1_000) && stopped() {
            return Err(Error::Stopped);
        }
        entries += 1;
        // Fallible copy, including the trailing NUL, before advancing DIR.
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(name.to_bytes_with_nul().len())
            .map_err(|_| Error::HostMemory)?;
        owned.extend_from_slice(name.to_bytes_with_nul());
        match directory_at(cursor.fd(), &owned) {
            Ok(file) => {
                if stack.len() == REMOVE_DEPTH {
                    return Err(Error::Internal);
                }
                stack.push(Cursor::new(file, Some(owned))?);
            }
            Err(error) => {
                if !matches!(error.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) {
                    return Err(error.into());
                }
                unlink_at(cursor.fd(), &owned, 0)?;
            }
        }
    }
    if stopped() {
        return Err(Error::Stopped);
    }
    unlink_at(parent.as_raw_fd(), &top, libc::AT_REMOVEDIR)?;
    Ok(())
}

fn directory_at(parent: libc::c_int, name: &[u8]) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    // SAFETY: callers supply a live directory fd and a NUL-terminated name.
    let fd = unsafe { libc::openat(parent, name.as_ptr().cast(), directory_flags()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned one newly owned descriptor.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}
fn unlink_at(parent: libc::c_int, name: &[u8], flags: libc::c_int) -> io::Result<()> {
    // SAFETY: live directory fd, NUL-terminated name; flags are 0 or AT_REMOVEDIR.
    if unsafe { libc::unlinkat(parent, name.as_ptr().cast(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

use std::{fs::File, os::fd::AsRawFd};
struct Cursor {
    dir: *mut libc::DIR,
    name: Option<Vec<u8>>,
}
impl Drop for Cursor {
    fn drop(&mut self) {
        // SAFETY: each cursor owns exactly one successful fdopendir.
        unsafe {
            libc::closedir(self.dir);
        }
    }
}
impl Cursor {
    fn new(file: File, name: Option<Vec<u8>>) -> io::Result<Self> {
        // SAFETY: file owns this directory fd; fdopendir borrows on
        // failure and takes ownership on success.
        let dir = unsafe { libc::fdopendir(file.as_raw_fd()) };
        if dir.is_null() {
            return Err(io::Error::last_os_error());
        }
        std::mem::forget(file);
        Ok(Self { dir, name })
    }
    fn fd(&self) -> libc::c_int {
        // SAFETY: dir remains owned and live until this cursor drops.
        unsafe { libc::dirfd(self.dir) }
    }
}
fn errno() -> *mut libc::c_int {
    #[cfg(target_os = "linux")]
    // SAFETY: libc supplies a valid thread-local errno location.
    unsafe {
        libc::__errno_location()
    }
    #[cfg(not(target_os = "linux"))]
    // SAFETY: libc supplies a valid thread-local errno location.
    unsafe {
        libc::__error()
    }
}

#[cfg(test)]
mod tests {
    use super::{INTERRUPTED_RETRIES, retry_interrupted};
    use std::io;

    #[test]
    fn an_interrupted_call_is_retried_a_bounded_number_of_times() {
        let mut calls = 0;
        let result: io::Result<()> = retry_interrupted(|| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::Interrupted))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(calls, INTERRUPTED_RETRIES + 1);

        let mut calls = 0;
        let result = retry_interrupted(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 3);

        let mut calls = 0;
        let result: io::Result<()> = retry_interrupted(|| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }
}
