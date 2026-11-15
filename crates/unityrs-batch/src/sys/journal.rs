//! Path-opened journal descriptors; all temporary path storage is fallible.
use std::io;
use std::os::fd::FromRawFd;
use std::path::Path;

pub(crate) fn journal_file(path: &Path, create: bool) -> io::Result<std::fs::File> {
    let path = super::terminated_path(path)?;
    let flags = libc::O_RDWR
        | libc::O_APPEND
        | libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NONBLOCK
        | if create {
            libc::O_CREAT | libc::O_EXCL
        } else {
            0
        };
    let fd = super::retry_interrupted(|| {
        // SAFETY: path is NUL-terminated and alive for open; create has a mode.
        let fd = unsafe { super::spool_open(path.as_ptr().cast(), flags, 0o600_u32) };
        if fd == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    })?;
    // SAFETY: open returned a new descriptor, transferred exactly once.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(file)
}
