use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};
pub(super) fn signal(pid: u32, signal: i32, group: bool) -> io::Result<()> {
    let pid = i32::try_from(pid).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: positive pid comes from a child we own; negative targets only
    // that child's dedicated process group. No memory is borrowed by kill.
    let rc = unsafe { libc::kill(if group { -pid } else { pid }, signal) };
    if rc == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}
pub(super) fn fifo(path: &Path) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: path is NUL terminated and alive throughout mkfifo.
    if unsafe { libc::mkfifo(path.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
/// Observe without reaping: kill the owned process group while the root's pid
/// still cannot be reused, then let Child collect the status exactly once.
pub(super) fn exited(pid: u32) -> io::Result<bool> {
    // SAFETY: siginfo_t contains only integer/pointer fields, all zero valid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: pid is our unreaped child, info is writable complete storage.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &raw mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: waitid initialized info; si_pid is the exited child's pid or 0.
    Ok(unsafe { info.si_pid() } != 0)
}
/// Blocking reap of an adopted child by pid, once `exited` saw it end (or
/// after a SIGKILL); `EINTR` is retried.
pub(super) fn reap(pid: u32) -> io::Result<std::process::ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    let pid = i32::try_from(pid).map_err(|_| io::ErrorKind::InvalidInput)?;
    loop {
        let mut status = 0;
        // SAFETY: pid is an adopted child this scene alone reaps; status is
        // writable for the call.
        if unsafe { libc::waitpid(pid, &raw mut status, 0) } == pid {
            return Ok(std::process::ExitStatus::from_raw(status));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
pub(super) fn cleanup_exited_group(pid: u32) -> io::Result<()> {
    match signal(pid, libc::SIGKILL, true) {
        // Darwin reports EPERM for a group containing only its unreaped zombie.
        // This helper is called only after WNOWAIT observed the exited root;
        // live engine/service group kills still propagate permission failures.
        #[cfg(target_os = "macos")]
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => Ok(()),
        result => result,
    }
}
