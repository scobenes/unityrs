//! Accounted, uncommitted artifacts. Ownership outlives a writer and never
//! returns claims on drop. Only successful explicit deletion returns space.
mod claims;
mod files;
pub use claims::Counter;
pub use files::{Directory, SpoolFile};

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Why the spool was full (`batch.md` 2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    SizeBytes,
    Reserve,
    Enospc,
    Edquot,
}
impl Full {
    /// Stable detail for the run failure `spool_full`.
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::SizeBytes => "size_bytes",
            Self::Reserve => "reserve",
            Self::Enospc => "ENOSPC",
            Self::Edquot => "EDQUOT",
        }
    }
}
/// Run-level failures; never an input's data failure.
#[derive(Debug)]
pub enum Error {
    /// `spool_full`, exit 5.
    Full(Full),
    /// `host_io`, exit 74. The error is for diagnostics, not public paths.
    HostIo(io::Error),
    /// Invalid accounting state or reuse of a failed/removed owner.
    Internal,
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.raw_os_error() {
            Some(libc::ENOSPC) => Self::Full(Full::Enospc),
            Some(libc::EDQUOT) => Self::Full(Full::Edquot),
            _ => Self::HostIo(e),
        }
    }
}

/// Injectable filesystem facts and operations. Fault implementations may
/// wrap a real file to fail a write or flush; production uses [`Host`].
/// Paths here are trusted prepared directories and generated basenames.
pub trait FileSystem: Send + Sync {
    /// Caller-available bytes, not root's free blocks.
    fn available(&self, directory: &Path) -> io::Result<u64>;
    /// Exclusive create: never truncate an earlier artifact.
    fn create(&self, path: &Path) -> io::Result<Box<dyn Write + Send>> {
        Host.create(path)
    }
    /// Create only the requested directory; its parent is prepared.
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }
    /// Rename preserves claims.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    /// Remove owned artifacts, without following directory symlinks.
    /// Partial deletion followed by error retains the entire claim.
    fn remove(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }
}
/// Production facts. No fsync per output: sealing completes writes and
/// closes files; crash durability belongs to later publication.
pub struct Host;
impl FileSystem for Host {
    fn available(&self, directory: &Path) -> io::Result<u64> {
        crate::sys::available_bytes(directory)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Write + Send>> {
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Box::new(f))
    }
}

/// A generated output name. It uses the plan ordinal, never sink arrival.
#[must_use]
pub fn output_name(plan_index: u64) -> PathBuf {
    PathBuf::from(format!("o{plan_index}"))
}
