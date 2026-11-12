//! `JOB.json`'s bytes (`docs/plans/batch.md` 2.11 step 2;
//! `docs/BATCH_SPEC.md` section 1).
//!
//! The file is opened `O_RDONLY | O_NONBLOCK | O_NOCTTY`, as a local
//! bundle is, so a FIFO cannot hold the open and a terminal cannot become
//! the engine's controlling terminal, and taken only when `fstat` shows a
//! regular file. It is read whole into a buffer sized by that `fstat`,
//! which never grows past 64 MiB + 1 byte, so a file that grows while it
//! is read is still caught and still bounded. The rules, in section 10's
//! order: `unreadable` (missing, or it cannot be opened or read),
//! `not_regular`, `too_large`.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use super::{BadSpec, MAX_SPEC_BYTES, Rule};

/// Opens `path` as `JOB.json` is opened.
fn open(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
}

/// The most bytes read: one past the cap shows a file over it.
const CAP: usize = (64 << 20) + 1;
const _: () = assert!(CAP as u64 == MAX_SPEC_BYTES + 1);

/// Reads `JOB.json` at `path`: its bytes, or `unreadable`, `not_regular`
/// or `too_large` (`docs/BATCH_SPEC.md` section 10).
pub fn read_job(path: &Path) -> Result<Vec<u8>, BadSpec> {
    let unreadable = || BadSpec::new(Rule::Unreadable, "");
    let mut file = open(path).map_err(|_| unreadable())?;
    let meta = file.metadata().map_err(|_| unreadable())?;
    if !meta.file_type().is_file() {
        return Err(BadSpec::new(Rule::NotRegular, ""));
    }
    if meta.len() > MAX_SPEC_BYTES {
        return Err(BadSpec::new(Rule::TooLarge, ""));
    }
    let buf = read_capped(&mut file, meta.len()).map_err(|_| unreadable())?;
    if buf.len() as u64 > MAX_SPEC_BYTES {
        return Err(BadSpec::new(Rule::TooLarge, ""));
    }
    Ok(buf)
}

/// Reads `file` to its end or to [`CAP`] bytes, into a buffer of the
/// `fstat` size + 1 that grows, only if the file grew, by doubling up to
/// [`CAP`] and never past it.
fn read_capped(file: &mut File, size: u64) -> std::io::Result<Vec<u8>> {
    let want = usize::try_from(size).map_or(CAP, |s| s.saturating_add(1).min(CAP));
    let mut buf: Vec<u8> = Vec::with_capacity(want);
    let mut chunk = vec![0u8; 64 << 10];
    while buf.len() < CAP {
        let room = (CAP - buf.len()).min(chunk.len());
        let n = match file.read(chunk.get_mut(..room).unwrap_or_default()) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.len() + n > buf.capacity() {
            let grow = buf.capacity().max(n).min(CAP - buf.len());
            buf.reserve_exact(grow);
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{CAP, read_capped};
    use std::io::Write;
    use unityrs_testkit::scratch::Scratch;

    #[test]
    fn a_file_that_grew_is_read_within_the_cap() {
        let dir = Scratch::new("batch-read");
        let path = dir.join("JOB.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![b' '; 3 << 20]).unwrap();
        drop(f);
        // As if `fstat` had said 10 bytes.
        let mut f = std::fs::File::open(&path).unwrap();
        let buf = read_capped(&mut f, 10).unwrap();
        assert_eq!(buf.len(), 3 << 20);
        assert!(buf.capacity() <= CAP);
    }
}
