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
    write_once(route, bytes, lifecycle).ok() == Some(bytes.len())
}

const RETRY_ATTEMPTS: usize = 1024;

/// Retry transient backpressure on main without allocating or spawning.
/// A partial write advances the slice; only an unfinished line is dropped.
/// At most 1,024 writes, including partial writes and interrupted attempts.
/// Transient errors pause for at most RETRY (1 ms), clipped to the original
/// deadline. Exhaustion leaves the pending line's single drop in place.
pub(crate) fn within(route: Route, bytes: &[u8], wait: std::time::Duration) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(hooks) = RETRY_HOOKS.get() {
        return within_with(bytes, wait, hooks.write, hooks.now, hooks.sleep);
    }
    within_with(
        bytes,
        wait,
        |bytes| write_once(route, bytes, false),
        std::time::Instant::now,
        std::thread::sleep,
    )
}

fn within_with(
    mut bytes: &[u8],
    wait: std::time::Duration,
    mut write: impl FnMut(&[u8]) -> io::Result<usize>,
    now: impl Fn() -> std::time::Instant,
    sleep: impl Fn(std::time::Duration),
) -> bool {
    let until = now() + wait;
    let mut attempts = 0;
    while !bytes.is_empty() {
        if attempts == RETRY_ATTEMPTS || now() >= until {
            return false;
        }
        attempts += 1;
        match write(bytes) {
            Ok(0) => return false,
            Ok(n) => bytes = bytes.get(n..).unwrap_or_default(),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                if attempts == RETRY_ATTEMPTS {
                    return false;
                }
                sleep(crate::progress::early::RETRY.min(until.saturating_duration_since(now())));
            }
            Err(_) => return false,
        }
    }
    true
}

/// Library-only startup-warning write/clock seam. Installed on the calling
/// thread alone; other diagnostic and lifecycle writes keep their real route.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
pub struct StderrRetryHooks {
    pub write: fn(&[u8]) -> io::Result<usize>,
    pub now: fn() -> std::time::Instant,
    pub sleep: fn(std::time::Duration),
    /// Called after the pending warning's drop accounting is settled.
    pub finished: fn(),
}
#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static RETRY_HOOKS: std::cell::Cell<Option<StderrRetryHooks>> = const { std::cell::Cell::new(None) };
}
#[cfg(any(test, feature = "test-hooks"))]
pub fn stderr_retry_hooks(hooks: Option<StderrRetryHooks>) {
    RETRY_HOOKS.set(hooks);
}
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn retry_finished() {
    if let Some(hooks) = RETRY_HOOKS.get() {
        (hooks.finished)();
    }
}

fn write_once(route: Route, bytes: &[u8], lifecycle: bool) -> io::Result<usize> {
    if matches!(route, Route::Drop) || lifecycle && matches!(route, Route::File) {
        return Err(io::Error::from(io::ErrorKind::Unsupported));
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
    usize::try_from(n).map_err(|_| io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };

    #[test]
    fn interrupted_backoff_is_clipped_to_the_remaining_budget() {
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let attempts = Cell::new(0);
        let pauses = Cell::new(0);
        assert!(!within_with(
            b"warning\n",
            Duration::from_millis(1),
            |_| {
                attempts.set(attempts.get() + 1);
                elapsed.set(Duration::from_micros(750));
                Err(io::ErrorKind::Interrupted.into())
            },
            || start + elapsed.get(),
            |pause| {
                assert_eq!(pause, Duration::from_micros(250));
                pauses.set(pauses.get() + 1);
                elapsed.set(elapsed.get() + pause);
            },
        ));
        assert_eq!(attempts.get(), 1, "no attempt past the deadline");
        assert_eq!(pauses.get(), 1);
    }

    #[test]
    fn partial_writes_advance_the_slice_and_also_obey_the_cap() {
        let now = Instant::now();
        let mut delivered = Vec::new();
        assert!(within_with(
            b"warning\n",
            Duration::from_secs(1),
            |bytes| {
                delivered.push(bytes[0]);
                Ok(1)
            },
            || now,
            |_| panic!("successful partial write needs no backoff"),
        ));
        assert_eq!(delivered, b"warning\n");
        let mut attempts = 0;
        assert!(!within_with(
            &[b'x'; 1025],
            Duration::from_secs(1),
            |_| {
                attempts += 1;
                assert!(attempts <= 1024);
                Ok(1)
            },
            || now,
            |_| panic!("successful partial write needs no backoff"),
        ));
        assert_eq!(attempts, 1024);
    }
}
