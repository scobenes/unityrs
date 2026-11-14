use std::{
    io,
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};
#[derive(Clone, Copy)]
pub enum Route {
    Ordinary,
    Socket,
    File,
    Drop,
}
#[allow(clippy::useless_conversion)] // dev_t is signed on macOS.
pub fn prepare() -> io::Result<Route> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat has writable storage for the whole fstat output.
    super::cvt(unsafe { libc::fstat(2, stat.as_mut_ptr()) })?;
    // SAFETY: successful fstat initialized all fields.
    let stat = unsafe { stat.assume_init() };
    let kind = stat.st_mode & libc::S_IFMT;
    if kind == libc::S_IFSOCK {
        return Ok(Route::Socket);
    }
    if kind == libc::S_IFREG || kind == libc::S_IFBLK {
        return Ok(Route::File);
    }
    if kind == libc::S_IFCHR
        && std::fs::metadata("/dev/null")
            .is_ok_and(|s| Some(s.rdev()) == u64::try_from(stat.st_rdev).ok())
    {
        return Ok(Route::Ordinary);
    }
    // Linux opens a new description, so O_NONBLOCK is this writer's own.
    // Darwin's `/dev/fd/2` open duplicates fd 2's existing description and
    // ignores the new flags, O_NONBLOCK included: on that non-normative test
    // host a write to a full caller pipe may block. Production is Linux only.
    #[cfg(target_os = "linux")]
    let path = "/proc/self/fd/2";
    #[cfg(not(target_os = "linux"))]
    let path = "/dev/fd/2";
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path);
    let (file, route) = match opened {
        Ok(file) => (file, Route::Ordinary),
        Err(_) => (
            std::fs::OpenOptions::new().write(true).open("/dev/null")?,
            Route::Drop,
        ),
    };
    // SAFETY: live newly opened descriptor; duplicating onto stderr does not
    // alter caller OFD flags. This is early single-threaded startup only.
    super::cvt(unsafe { libc::dup2(file.as_raw_fd(), 2) })?;
    Ok(route)
}
pub fn write(route: Route, bytes: &[u8], lifecycle: bool) -> bool {
    if matches!(route, Route::Drop) || lifecycle && matches!(route, Route::File) {
        return false;
    }
    // SAFETY: bytes is readable for its size, at most PIPE_BUF. One syscall,
    // with nonblocking send for sockets. Lifecycle never reaches file writes.
    let n = unsafe {
        if matches!(route, Route::Socket) {
            libc::send(2, bytes.as_ptr().cast(), bytes.len(), libc::MSG_DONTWAIT)
        } else {
            libc::write(2, bytes.as_ptr().cast(), bytes.len())
        }
    };
    usize::try_from(n).ok() == Some(bytes.len())
}
