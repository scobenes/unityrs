//! The engine's few calls into the C library that std does not offer, each
//! a single `libc` call under a `// SAFETY:` comment: with
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
