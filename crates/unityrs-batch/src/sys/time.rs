//! Wall-clock time, rendered without allocation.
use std::io;

/// UTC wall time for manifest timestamps, in fixed RFC 3339 storage.
pub fn utc_now() -> io::Result<[u8; 20]> {
    use std::io::Write;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let seconds = libc::time_t::try_from(seconds).map_err(io::Error::other)?;
    let mut broken = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: seconds is readable and broken has writable storage for tm;
    // gmtime_r uses caller-owned output and does not retain either pointer.
    if unsafe { libc::gmtime_r(&raw const seconds, broken.as_mut_ptr()) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful gmtime_r initialized every field of broken.
    let broken = unsafe { broken.assume_init() };
    let year = broken
        .tm_year
        .checked_add(1900)
        .filter(|y| (0..=9999).contains(y))
        .ok_or(io::ErrorKind::InvalidData)?;
    let mut bytes = [0; 20];
    write!(
        &mut bytes.as_mut_slice(),
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        broken.tm_mon + 1,
        broken.tm_mday,
        broken.tm_hour,
        broken.tm_min,
        broken.tm_sec
    )?;
    Ok(bytes)
}
