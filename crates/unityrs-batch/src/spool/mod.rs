//! Accounted, uncommitted artifacts. Ownership outlives a writer and never
//! returns claims on drop. Only successful explicit deletion returns space.
mod claims;
mod files;
mod owned_file;
pub(crate) mod path;
pub mod random;
pub mod workspace;
pub use claims::Counter;
pub use files::{Directory, DirectoryClaim, SpoolFile};
pub use owned_file::{FileClaim, OwnedFile};

use std::io::{self, Write};
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
    /// Cooperative cancellation before a write or final flush. Claims remain owned.
    Stopped,
    /// `spool_full`, exit 5.
    Full(Full),
    /// `host_io`, exit 74. The error is for diagnostics, not public paths.
    HostIo(io::Error),
    /// A fallible path or file-adapter allocation was refused.
    HostMemory,
    /// Invalid accounting state or reuse of a failed/removed owner.
    Internal,
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.raw_os_error() {
            Some(libc::ENOSPC) => Self::Full(Full::Enospc),
            Some(libc::EDQUOT) => Self::Full(Full::Edquot),
            _ if e.kind() == io::ErrorKind::OutOfMemory => Self::HostMemory,
            _ => Self::HostIo(e),
        }
    }
}

/// A spool writer that sets file permissions through its original descriptor.
pub trait Writer: Write + Send {
    /// Apply mode `0400` without reopening a path or depending on the umask.
    fn seal(&self) -> io::Result<()>;
}
impl Writer for std::fs::File {
    fn seal(&self) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        self.set_permissions(std::fs::Permissions::from_mode(0o400))
    }
}

/// Injectable filesystem facts and operations. Fault implementations may
/// wrap a real file to fail a write or flush; production uses [`Host`].
/// Paths here are trusted prepared directories and generated basenames.
pub trait FileSystem: Send + Sync {
    fn create_random(&self, path: &Path) -> io::Result<Box<dyn random::Random>> {
        Ok(crate::alloc::try_box(crate::sys::spool_file(path, true)?)?)
    }
    /// Open an immutable internal artifact. Injectable for bounded read faults.
    fn open(&self, path: &Path) -> io::Result<Box<dyn std::io::Read + Send>> {
        let file = crate::sys::spool_file(path, false)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(crate::alloc::try_box(file)?)
    }
    /// Used only to check sealed descriptors, never to reconstruct a claim.
    fn length(&self, path: &Path) -> io::Result<u64> {
        crate::sys::spool_length(path)
    }
    /// Caller-available bytes, not root's free blocks.
    fn available(&self, directory: &Path) -> io::Result<u64>;
    /// Exclusive create: never truncate an earlier artifact.
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        Host.create(path)
    }
    /// Create only the requested directory; its parent is prepared.
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        crate::sys::spool_mkdir(path)
    }
    /// Rename preserves claims.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        crate::sys::spool_rename(from, to)
    }
    /// Remove owned artifacts, without following directory symlinks.
    /// Partial deletion followed by error retains the entire claim.
    fn remove(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }
    /// Stop-aware descriptor traversal; partial deletion retains all claims.
    fn remove_stoppable(
        &self,
        root: &std::fs::File,
        path: &Path,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        crate::sys::remove_tree(root, path, stopped)
    }
    /// Delete exactly one owned staged/peek file; never traverse a directory.
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        crate::sys::spool_unlink(path)
    }
}
/// Production facts. No fsync per output: sealing completes writes and
/// closes files; crash durability belongs to later publication.
pub struct Host;
impl FileSystem for Host {
    fn available(&self, directory: &Path) -> io::Result<u64> {
        crate::sys::available_bytes(directory)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        let f = crate::sys::spool_file(path, true)?;
        Ok(crate::alloc::try_box(f)?)
    }
}

/// A generated output name. It uses the plan ordinal, never sink arrival.
#[must_use]
pub fn output_name(plan_index: u64) -> PathBuf {
    PathBuf::from(format!("o{plan_index}"))
}
