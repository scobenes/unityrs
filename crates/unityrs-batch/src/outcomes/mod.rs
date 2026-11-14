//! Fixed positional outcomes. Only complete records advertise results;
//! archive success metadata precedes the final status byte under the commit gate.
use crate::{
    phase::Error,
    spool::{
        self, Counter, FileSystem,
        random::{Mutable, Random},
    },
};
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Code {
    #[default]
    NotWritten = 0,
    Written = 1,
    Present = 2,
    PublishFailed = 3,
    PublishMismatch = 4,
    CredentialsExpired = 5,
    ArchiveTooLarge = 6,
}
impl Code {
    fn decode(n: u8, archive: bool) -> Result<Self, Error> {
        match n {
            0 => Ok(Self::NotWritten),
            1 => Ok(Self::Written),
            2 => Ok(Self::Present),
            3 => Ok(Self::PublishFailed),
            4 => Ok(Self::PublishMismatch),
            5 => Ok(Self::CredentialsExpired),
            6 if archive => Ok(Self::ArchiveTooLarge),
            _ => Err(Error::internal()),
        }
    }
    #[must_use]
    pub const fn status(self) -> &'static str {
        match self {
            Self::NotWritten => "not_written",
            Self::Written => "written",
            Self::Present => "present",
            _ => "failed",
        }
    }
    #[must_use]
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::NotWritten | Self::Written | Self::Present => None,
            Self::PublishFailed => Some("publish_failed"),
            Self::PublishMismatch => Some("publish_mismatch"),
            Self::CredentialsExpired => Some("credentials_expired"),
            Self::ArchiveTooLarge => Some("archive_too_large"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Archive {
    pub code: Code,
    pub size: u64,
    pub md5: [u8; 16],
    etag: [u8; 100],
    etag_length: u8,
}
impl Default for Archive {
    fn default() -> Self {
        Self {
            code: Code::NotWritten,
            size: 0,
            md5: [0; 16],
            etag: [0; 100],
            etag_length: 0,
        }
    }
}
impl Archive {
    pub fn success(
        code: Code,
        size: u64,
        md5: [u8; 16],
        etag: Option<&str>,
    ) -> Result<Self, Error> {
        if !matches!(code, Code::Written | Code::Present) {
            return Err(Error::internal());
        }
        let mut out = Self {
            code,
            size,
            md5,
            ..Self::default()
        };
        if let Some(text) = etag {
            if text.len() > 100 || text.bytes().any(|b| b == b'"' || b == 0) {
                return Err(Error::internal());
            }
            out.etag
                .get_mut(..text.len())
                .ok_or_else(Error::internal)?
                .copy_from_slice(text.as_bytes());
            out.etag_length = u8::try_from(text.len()).map_err(|_| Error::internal())?;
        }
        Ok(out)
    }
    pub fn failed(code: Code) -> Result<Self, Error> {
        if !matches!(
            code,
            Code::PublishFailed
                | Code::PublishMismatch
                | Code::CredentialsExpired
                | Code::ArchiveTooLarge
        ) {
            return Err(Error::internal());
        }
        Ok(Self {
            code,
            ..Self::default()
        })
    }
    pub fn etag(&self) -> Result<Option<&str>, Error> {
        if self.etag_length == 0 {
            return Ok(None);
        }
        std::str::from_utf8(
            self.etag
                .get(..usize::from(self.etag_length))
                .ok_or_else(Error::internal)?,
        )
        .map(Some)
        .map_err(|_| Error::internal())
    }
}
pub fn encode(record: &Archive) -> Result<[u8; 128], Error> {
    let mut bytes = [0; 128];
    *bytes.first_mut().ok_or_else(Error::internal)? = record.code as u8;
    if matches!(record.code, Code::Written | Code::Present) {
        *bytes.get_mut(1).ok_or_else(Error::internal)? = record.etag_length;
        bytes
            .get_mut(2..10)
            .ok_or_else(Error::internal)?
            .copy_from_slice(&record.size.to_le_bytes());
        bytes
            .get_mut(10..26)
            .ok_or_else(Error::internal)?
            .copy_from_slice(&record.md5);
        bytes
            .get_mut(26..126)
            .ok_or_else(Error::internal)?
            .copy_from_slice(&record.etag);
    }
    Ok(bytes)
}
fn read(file: Option<&dyn Random>, mut offset: u64, mut bytes: &mut [u8]) -> Result<bool, Error> {
    let Some(file) = file else { return Ok(false) };
    while !bytes.is_empty() {
        let n = crate::sys::retry_interrupted(|| file.read_at(bytes, offset))?;
        if n == 0 {
            return Ok(false);
        }
        if n > bytes.len() {
            return Err(Error::internal());
        }
        offset = offset
            .checked_add(u64::try_from(n).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        bytes = bytes.get_mut(n..).ok_or_else(Error::internal)?;
    }
    Ok(true)
}
pub fn read_loose(file: Option<&dyn Random>, index: u64) -> Result<Code, Error> {
    let mut byte = [0];
    if !read(file, index, &mut byte)? {
        return Ok(Code::NotWritten);
    }
    Code::decode(*byte.first().ok_or_else(Error::internal)?, false)
}
pub fn read_archive(file: Option<&dyn Random>, index: u64) -> Result<Archive, Error> {
    if file.is_none() {
        return Ok(Archive::default());
    }
    let offset = index.checked_mul(128).ok_or_else(Error::internal)?;
    let mut bytes = [0; 128];
    if !read(file, offset, &mut bytes)? {
        return Ok(Archive::default());
    }
    let code = Code::decode(*bytes.first().ok_or_else(Error::internal)?, true)?;
    if !matches!(code, Code::Written | Code::Present) {
        return if code == Code::NotWritten {
            Ok(Archive::default())
        } else {
            Archive::failed(code)
        };
    }
    let n = usize::from(*bytes.get(1).ok_or_else(Error::internal)?);
    if n > 100
        || bytes
            .get(26 + n..)
            .ok_or_else(Error::internal)?
            .iter()
            .any(|&b| b != 0)
    {
        return Err(Error::internal());
    }
    let text = std::str::from_utf8(bytes.get(26..26 + n).ok_or_else(Error::internal)?)
        .map_err(|_| Error::internal())?;
    Archive::success(
        code,
        u64::from_le_bytes(
            bytes
                .get(2..10)
                .ok_or_else(Error::internal)?
                .try_into()
                .map_err(|_| Error::internal())?,
        ),
        bytes
            .get(10..26)
            .ok_or_else(Error::internal)?
            .try_into()
            .map_err(|_| Error::internal())?,
        if n == 0 { None } else { Some(text) },
    )
}
/// Outcome reads: the run's own files, or a view of what a stop left.
pub trait Read: Sync {
    fn loose(&self, index: u64) -> Result<Code, Error>;
    fn archive(&self, index: u64) -> Result<Archive, Error>;
}
/// A read view over outcome files a stop may have left missing or short
/// (their zero-writes interrupted): a missing file, or a record past its
/// end, reads `not_written`. Indices are still checked against the plan.
pub struct View {
    loose: Option<Box<dyn Random>>,
    archives: Option<Box<dyn Random>>,
    winners: u64,
    count: u64,
}
impl View {
    pub fn open(
        root: &Path,
        winners: u64,
        archives: u64,
        fs: &dyn FileSystem,
    ) -> Result<Self, Error> {
        let open = |archive: bool| match fs.open_random(&spool::random::location(root, archive)?) {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::from(spool::Error::from(e))),
        };
        Ok(Self {
            loose: open(false)?,
            archives: open(true)?,
            winners,
            count: archives,
        })
    }
}
impl Read for View {
    fn loose(&self, index: u64) -> Result<Code, Error> {
        if index >= self.winners {
            return Err(Error::internal());
        }
        read_loose(self.loose.as_deref(), index)
    }
    fn archive(&self, index: u64) -> Result<Archive, Error> {
        if index >= self.count {
            return Err(Error::internal());
        }
        read_archive(self.archives.as_deref(), index)
    }
}
impl Read for Outcomes {
    fn loose(&self, index: u64) -> Result<Code, Error> {
        Outcomes::loose(self, index)
    }
    fn archive(&self, index: u64) -> Result<Archive, Error> {
        Outcomes::archive(self, index)
    }
}
pub struct Outcomes {
    loose: Mutable,
    archives: Mutable,
    winners: u64,
    count: u64,
}
impl Outcomes {
    pub fn create(
        root: &Path,
        winners: u64,
        archives: u64,
        counter: &Arc<Counter>,
        fs: &dyn FileSystem,
        zeros: &[u8],
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        let length = archives.checked_mul(128).ok_or_else(Error::internal)?;
        let loose = Mutable::create(
            &spool::random::location(root, false)?,
            winners,
            counter,
            fs,
            zeros,
            stopped,
        )?;
        let archives = Mutable::create(
            &spool::random::location(root, true)?,
            length,
            counter,
            fs,
            zeros,
            stopped,
        )?;
        Ok(Self {
            loose,
            archives,
            winners,
            count: length / 128,
        })
    }
    pub fn loose(&self, index: u64) -> Result<Code, Error> {
        if index >= self.winners {
            return Err(Error::internal());
        }
        read_loose(Some(&self.loose), index)
    }
    pub fn archive(&self, index: u64) -> Result<Archive, Error> {
        if index >= self.count {
            return Err(Error::internal());
        }
        read_archive(Some(&self.archives), index)
    }
    pub fn set(&self, index: u64, code: Code) -> Result<(), Error> {
        if index >= self.winners || code == Code::ArchiveTooLarge {
            return Err(Error::internal());
        }
        self.loose.write(index, &[code as u8])?;
        Ok(())
    }
    pub fn set_archive(&self, index: u64, value: &Archive) -> Result<(), Error> {
        if index >= self.count || value.code == Code::NotWritten {
            return Err(Error::internal());
        }
        let offset = index.checked_mul(128).ok_or_else(Error::internal)?;
        if !matches!(value.code, Code::Written | Code::Present) {
            self.archives.write(offset, &[value.code as u8])?;
            return Ok(());
        }
        // Acknowledged success metadata is immutable. Verification downgrades
        // only the status; a failed downgrade must leave the old metadata intact.
        if self.archive(index)?.code != Code::NotWritten {
            return Err(Error::internal());
        }
        let bytes = encode(value)?;
        self.archives
            .write(offset + 1, bytes.get(1..).ok_or_else(Error::internal)?)?;
        self.archives
            .write(offset, bytes.get(..1).ok_or_else(Error::internal)?)?;
        Ok(())
    }
}
