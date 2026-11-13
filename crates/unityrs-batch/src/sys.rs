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

use std::{io, time::Duration};

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

/// The spool's exclusive read/write create (also for positional outcomes) or
/// no-follow immutable read, with fallible path storage.
pub(crate) fn spool_file(path: &std::path::Path, create: bool) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    let path = terminated_path(path)?;
    let flags = libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | if create {
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
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

/// The device and inode of the directory `path` names, without following a
/// final symlink and with fallible path storage; anything but a directory
/// is `InvalidData`.
pub(crate) fn directory_identity(path: &std::path::Path) -> io::Result<(u64, u64)> {
    let path = terminated_path(path)?;
    let mut stat = std::mem::MaybeUninit::<SpoolStat>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable storage for
    // one stat. Both remain alive throughout lstat.
    if unsafe { spool_lstat(path.as_ptr().cast(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful lstat initialized the complete structure.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    // As std's `MetadataExt::dev` and `ino` do: `dev_t` is `i32` on macOS.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        clippy::unnecessary_cast,
        clippy::useless_conversion
    )]
    Ok((stat.st_dev as u64, stat.st_ino as u64))
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
/// Exclusive nonblocking lock. The inode must never be unlinked.
pub fn lock_exclusive(file: &File) -> io::Result<()> {
    // SAFETY: live descriptor and scalar flock flags; no borrowed memory.
    cvt(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) })
}
fn cvt(rc: i32) -> io::Result<()> {
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
/// Rooted single-component open. Only preflight selects the fallback.
pub fn rooted_open(
    parent: &File,
    name: &std::ffi::CStr,
    flags: i32,
    beneath: bool,
) -> io::Result<File> {
    use std::os::fd::FromRawFd;
    #[cfg(target_os = "linux")]
    if beneath {
        // SAFETY: open_how contains only unsigned integers, all zero bit
        // patterns valid; zeroing also covers future reserved fields.
        let mut how: libc::open_how = unsafe { std::mem::zeroed() };
        how.flags = u64::try_from(flags | libc::O_CLOEXEC)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        how.mode = if flags & libc::O_CREAT == 0 { 0 } else { 0o600 };
        how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_XDEV;
        // SAFETY: live parent, valid C string and initialized open_how of
        // the exact size passed; successful syscall transfers one new fd.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                parent.as_raw_fd(),
                name.as_ptr(),
                &raw const how,
                size_of::<libc::open_how>(),
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = i32::try_from(fd).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
        // SAFETY: fd is newly owned, transferred exactly once to File.
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = beneath;
    // SAFETY: live parent and C string; mode is supplied for O_CREAT.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            0o600_u32,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat transferred one newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn rooted_mkdir(parent: &File, name: &std::ffi::CStr) -> io::Result<()> {
    // SAFETY: parent is live, name a valid C string; mkdirat retains neither.
    cvt(unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) })
}
pub fn rooted_unlink(parent: &File, name: &std::ffi::CStr) -> io::Result<()> {
    // SAFETY: live directory and C string. Flags 0 never recurse or follow.
    cvt(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) })
}
pub fn rooted_rename(parent: &File, from: &std::ffi::CStr, to: &std::ffi::CStr) -> io::Result<()> {
    // SAFETY: live directory and two C strings; renameat retains no pointers.
    cvt(unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    })
}
#[path = "sys/entry.rs"]
mod entry;
pub use entry::{
    Entry, beneath_probe, reopen_pinned, resume_flags, rooted_entry, traversal_flags,
    visit_directory,
};
pub fn mount_id(file: &File) -> io::Result<Option<u64>> {
    #[cfg(target_os = "linux")]
    {
        let mut stat = std::mem::MaybeUninit::<libc::statx>::uninit();
        // SAFETY: empty C string with AT_EMPTY_PATH queries the live fd;
        // stat is writable for the complete output structure.
        cvt(unsafe {
            libc::statx(
                file.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_MNT_ID,
                stat.as_mut_ptr(),
            )
        })?;
        // SAFETY: successful statx initializes all output fields.
        let stat = unsafe { stat.assume_init() };
        Ok((stat.stx_mask & libc::STATX_MNT_ID != 0).then_some(stat.stx_mnt_id))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}
pub fn kernel_release() -> io::Result<(u32, u32)> {
    #[cfg(target_os = "linux")]
    {
        let mut name = std::mem::MaybeUninit::<libc::utsname>::uninit();
        // SAFETY: name has writable storage for the complete utsname.
        cvt(unsafe { libc::uname(name.as_mut_ptr()) })?;
        // SAFETY: uname initialized name, including a terminated release.
        let name = unsafe { name.assume_init() };
        // SAFETY: successful uname guarantees a NUL-terminated release.
        let text = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) }
            .to_str()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
        let mut parts = text.split('.');
        let major = parts.next().and_then(|s| s.parse().ok());
        let minor = parts.next().and_then(|s| s.parse().ok());
        major
            .zip(minor)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}
pub fn sync_filesystem(lock: &File) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: the lock descriptor has remained open since preflight.
    {
        cvt(unsafe { libc::syncfs(lock.as_raw_fd()) })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = lock;
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}
pub fn writeback(file: &File) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: live file, offset 0 and length 0 mean the whole file.
    {
        cvt(unsafe { libc::sync_file_range(file.as_raw_fd(), 0, 0, libc::SYNC_FILE_RANGE_WRITE) })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

#[cfg(test)]
#[path = "sys/tests.rs"]
mod tests;

#[path = "sys/remove.rs"]
mod remove;
pub use remove::remove_tree;
impl Cursor {
    fn next_name(&self) -> io::Result<Option<&std::ffi::CStr>> {
        // SAFETY: errno is writable TLS and dir remains live. readdir's
        // result is valid until the next call; callers consume it first.
        let entry = unsafe {
            *errno() = 0;
            libc::readdir(self.dir)
        };
        if entry.is_null() {
            // SAFETY: the TLS errno pointer remains valid on this thread.
            let error = unsafe { *errno() };
            return if error == 0 {
                Ok(None)
            } else {
                Err(io::Error::from_raw_os_error(error))
            };
        }
        // SAFETY: non-null readdir result contains a terminated d_name.
        Ok(Some(unsafe {
            std::ffi::CStr::from_ptr((*entry).d_name.as_ptr())
        }))
    }
}
pub fn canonical(path: &std::path::Path) -> io::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let name = terminated_path(path)?;
    let capacity = usize::try_from(libc::PATH_MAX)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?
        + 1;
    let mut bytes = Vec::<u8>::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    bytes.resize(capacity, 0);
    // SAFETY: name is terminated; bytes holds at least PATH_MAX+1 writable
    // bytes, as realpath requires, with no overlapping input/output memory.
    if unsafe { libc::realpath(name.as_ptr().cast(), bytes.as_mut_ptr().cast()) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    let n = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    bytes.truncate(n);
    Ok(std::ffi::OsString::from_vec(bytes).into())
}
pub fn available_at(file: &File) -> io::Result<u64> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: live fd and writable storage for the whole statvfs output.
    cvt(unsafe { libc::fstatvfs(file.as_raw_fd(), stat.as_mut_ptr()) })?;
    // SAFETY: successful fstatvfs initialized every output field.
    let stat = unsafe { stat.assume_init() };
    #[allow(clippy::useless_conversion)] // libc widths differ across hosts.
    u64::from(stat.f_bavail)
        .checked_mul(u64::from(stat.f_frsize))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
}
pub fn in_memory(file: &File) -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    {
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: live fd and writable storage for the complete statfs.
        cvt(unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) })?;
        // SAFETY: successful fstatfs initialized every output field.
        let stat = unsafe { stat.assume_init() };
        let kind = i128::from(stat.f_type) & 0xffff_ffff;
        // RAMFS_MAGIC is Linux UAPI's value; libc does not export it.
        Ok(kind == i128::from(libc::TMPFS_MAGIC) || kind == 0x8584_58f6)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Ok(false)
    }
}

/// Read a resource limit, or raise its soft limit to the supplied hard limit.
pub fn resource_limit(
    resource: crate::start::host::Resource,
    raise: Option<crate::start::host::Limit>,
) -> io::Result<crate::start::host::Limit> {
    use crate::start::host::{Limit, Resource};
    let kind = match resource {
        Resource::AddressSpace => libc::RLIMIT_AS,
        Resource::Data => libc::RLIMIT_DATA,
        Resource::OpenFiles => libc::RLIMIT_NOFILE,
        Resource::FileSize => libc::RLIMIT_FSIZE,
        Resource::Processes => libc::RLIMIT_NPROC,
    };
    let mut limits = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if let Some(limit) = raise {
        limits.rlim_cur = limit.hard.unwrap_or(libc::RLIM_INFINITY);
        limits.rlim_max = limits.rlim_cur;
        // SAFETY: valid resource id and fully initialized rlimit input.
        cvt(unsafe { libc::setrlimit(kind, &raw const limits) })?;
    } else {
        // SAFETY: valid resource id and writable rlimit output storage.
        cvt(unsafe { libc::getrlimit(kind, &raw mut limits) })?;
    }
    Ok(Limit {
        soft: (limits.rlim_cur != libc::RLIM_INFINITY).then_some(limits.rlim_cur),
        hard: (limits.rlim_max != libc::RLIM_INFINITY).then_some(limits.rlim_max),
    })
}
/// Initial-namespace exemption is checked separately against `uid_map`.
pub fn privileged() -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        let mut header = Header {
            version: 0x2008_0522,
            pid: 0,
        };
        let mut data = [Data {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        // SAFETY: version 3 capget writes two initialized Data elements and
        // may update header; both buffers have the required C layouts.
        let rc = unsafe { libc::syscall(libc::SYS_capget, &raw mut header, data.as_mut_ptr()) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getuid has no inputs or borrowed memory.
        Ok(unsafe { libc::getuid() } == 0
            || data
                .first()
                .is_some_and(|d| d.effective & ((1 << 21) | (1 << 24)) != 0))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

#[path = "sys/paths.rs"]
mod paths;
pub(crate) use paths::retry_interrupted;
use paths::terminated_path;
