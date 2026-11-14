//! Bounded procfs reads for the progress writer. Paths and read storage
//! never allocate, including when the writer starts late or procfs fails.
use std::{
    ffi::CStr,
    fs::File,
    io::{self, Read},
    os::fd::FromRawFd,
};

fn open(path: &CStr) -> io::Result<File> {
    let fd = super::retry_interrupted(|| {
        // SAFETY: path is a terminated static string; open retains no pointer.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    })?;
    // SAFETY: successful open transfers one new descriptor, owned here once.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn proc_stat(buffer: &mut [u8]) -> io::Result<usize> {
    read_bounded(&mut open(c"/proc/self/stat")?, buffer)
}
fn read_bounded(file: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut used = 0;
    while let Some(rest) = buffer.get_mut(used..).filter(|s| !s.is_empty()) {
        let n = super::retry_interrupted(|| file.read(rest))?;
        if n == 0 {
            return Ok(used);
        }
        used += n;
    }
    // A full buffer can be complete; one stack byte distinguishes truncation.
    let mut extra = [0];
    if super::retry_interrupted(|| file.read(&mut extra))? == 0 {
        Ok(used)
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

pub(crate) fn proc_memory(field: &str) -> u64 {
    use std::io::Read;
    let Ok(mut file) = open(c"/proc/self/status") else {
        return 0;
    };
    let mut block = [0; 1024];
    let mut line = [0; 256];
    let mut used = 0;
    loop {
        let Ok(n) = crate::sys::retry_interrupted(|| file.read(&mut block)) else {
            return 0;
        };
        if n == 0 {
            return 0;
        }
        for byte in block.get(..n).unwrap_or_default() {
            if *byte == b'\n' {
                if let Some(value) = line
                    .get(..used)
                    .and_then(|s| std::str::from_utf8(s).ok())
                    .and_then(|s| s.strip_prefix(field))
                {
                    return value
                        .trim()
                        .strip_suffix("kB")
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .map_or(0, |kb| kb.saturating_mul(1024));
                }
                used = 0;
            } else {
                if let Some(slot) = line.get_mut(used) {
                    *slot = *byte;
                }
                used = used.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_read_accepts_exact_fit_and_rejects_truncation() {
        for (mut input, expected) in [
            (&b"abc"[..], Ok(3)),
            (&b"abcd"[..], Ok(4)),
            (&b"abcde"[..], Err(io::ErrorKind::InvalidData)),
        ] {
            let original = input;
            let mut buffer = [0; 4];
            let result = read_bounded(&mut input, &mut buffer).map_err(|e| e.kind());
            assert_eq!(result, expected);
            if let Ok(n) = result {
                assert_eq!(&buffer[..n], original);
            }
        }
    }

    #[test]
    fn clock_and_rss_procfs_reads_do_not_allocate_under_refusal() {
        use crate::test_alloc::REFUSE;
        REFUSE.with(|r| r.set(1));
        let clock = crate::progress::clock::Clock::process(std::time::Instant::now());
        let rss = crate::progress::rss();
        let remaining = REFUSE.with(|r| r.replace(0));
        assert_eq!(remaining, 1, "procfs reads attempted a heap allocation");
        #[cfg(target_os = "linux")]
        {
            assert!(clock.is_boot_clock());
            assert!(rss > 0);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (clock, rss);
    }
}
