//! Fallible path storage and bounded interrupted-call retries.
use std::io;

// std's Unix path adapters can allocate infallibly for long paths. Keep
// these phase-path calls behind one fallible NUL-termination helper.
pub(super) fn terminated_path(path: &std::path::Path) -> io::Result<Vec<u8>> {
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
