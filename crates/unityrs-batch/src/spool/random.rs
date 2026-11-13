//! Accounted positional file. Reserve and zero before exposing updates;
//! mutations never acquire another claim and Drop never releases one.
use super::{Counter, Error, FileSystem, Full};
use std::os::unix::fs::FileExt;
use std::{io, path::PathBuf, sync::Arc};

pub trait Random: Send + Sync {
    fn available(&self) -> io::Result<u64>;
    fn read_at(&self, bytes: &mut [u8], offset: u64) -> io::Result<usize>;
    fn write_at(&self, bytes: &[u8], offset: u64) -> io::Result<usize>;
}
impl Random for std::fs::File {
    fn available(&self) -> io::Result<u64> {
        crate::sys::available_at(self)
    }
    fn read_at(&self, b: &mut [u8], offset: u64) -> io::Result<usize> {
        FileExt::read_at(self, b, offset)
    }
    fn write_at(&self, b: &[u8], offset: u64) -> io::Result<usize> {
        FileExt::write_at(self, b, offset)
    }
}
pub struct Mutable {
    file: Box<dyn Random>,
    length: u64,
    claimed: u64,
    reserve: u64,
}
impl Mutable {
    pub fn create(
        path: &std::path::Path,
        length: u64,
        counter: &Arc<Counter>,
        fs: &dyn FileSystem,
        zeros: &[u8],
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        if zeros.is_empty() || zeros.iter().any(|&b| b != 0) {
            return Err(Error::Internal);
        }
        if stopped() {
            return Err(Error::Stopped);
        }
        let file = fs.create_random(path)?;
        if fs.available(path.parent().ok_or(Error::Internal)?)? < counter.reserve {
            return Err(Error::Full(Full::Reserve));
        }
        let claimed = counter.increase(0, length)?;
        counter.claim(claimed)?;
        let out = Self {
            file,
            length,
            claimed,
            reserve: counter.reserve,
        };
        let mut offset = 0;
        while offset < length {
            if stopped() {
                return Err(Error::Stopped);
            }
            let n = usize::try_from(
                (length - offset).min(u64::try_from(zeros.len()).map_err(|_| Error::Internal)?),
            )
            .map_err(|_| Error::Internal)?;
            out.write(offset, zeros.get(..n).ok_or(Error::Internal)?)?;
            offset += u64::try_from(n).map_err(|_| Error::Internal)?;
        }
        Ok(out)
    }
    #[must_use]
    pub const fn claimed(&self) -> u64 {
        self.claimed
    }
    pub fn write(&self, mut offset: u64, mut bytes: &[u8]) -> Result<(), Error> {
        if self.file.available()? < self.reserve {
            return Err(Error::Full(Full::Reserve));
        }
        if offset
            .checked_add(u64::try_from(bytes.len()).map_err(|_| Error::Internal)?)
            .is_none_or(|n| n > self.length)
        {
            return Err(Error::Internal);
        }
        while !bytes.is_empty() {
            let n = crate::sys::retry_interrupted(|| self.file.write_at(bytes, offset))?;
            if n == 0 || n > bytes.len() {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            }
            offset += u64::try_from(n).map_err(|_| Error::Internal)?;
            bytes = bytes.get(n..).ok_or(Error::Internal)?;
        }
        Ok(())
    }
}
/// Generated outcomes paths only; allocation is included in phase overhead.
pub fn location(root: &std::path::Path, archive: bool) -> Result<PathBuf, Error> {
    super::path::join(
        root,
        std::path::Path::new(if archive {
            "archive_outcomes"
        } else {
            "outcomes"
        }),
    )
}
impl Random for Mutable {
    fn available(&self) -> io::Result<u64> {
        self.file.available()
    }
    fn read_at(&self, b: &mut [u8], offset: u64) -> io::Result<usize> {
        self.file.read_at(b, offset)
    }
    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}
