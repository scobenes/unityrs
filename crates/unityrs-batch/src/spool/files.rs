use super::{Counter, Error, FileSystem, Full, Writer};
use crate::sys::Shared;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A committed directory needs only its claim; its input index derives the
/// location under `done/`. Dropping this fact never returns disk space.
#[derive(Clone, Copy, Debug)]
pub struct DirectoryClaim {
    pub claimed_bytes: u64,
}

struct Ledger {
    counter: Arc<Counter>,
    claimed: AtomicU64,
}
/// Owner of exactly one newly created directory. Keep it through failure
/// cleanup or hand it to the controller; dropping it retains disk claims.
/// It cannot be removed or renamed while a writer still holds it.
pub struct Directory {
    path: PathBuf,
    removal_root: Arc<std::fs::File>,
    relative: PathBuf,
    ledger: Shared<Ledger>,
    fs: Arc<dyn FileSystem>,
    removed: bool,
}
impl Directory {
    pub fn into_claim(self) -> Result<DirectoryClaim, Error> {
        self.idle()?;
        Ok(DirectoryClaim {
            claimed_bytes: self.claimed(),
        })
    }

    /// Parent must already exist; an existing directory is never adopted.
    pub fn create(
        path: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> Result<Self, Error> {
        // Standalone owners pin the containing area's parent. Controllers
        // supply the spool's already held descriptor through create_anchored.
        let parent = path.parent().ok_or(Error::Internal)?;
        let root = parent
            .parent()
            .filter(|_| parent.file_name().is_some())
            .unwrap_or(parent);
        let relative = super::path::join(
            Path::new(""),
            path.strip_prefix(root).map_err(|_| Error::Internal)?,
        )?;
        let root = if root.as_os_str().is_empty() {
            Path::new(".")
        } else {
            root
        };
        let removal_root = Arc::new(crate::sys::open_directory(root)?);
        Self::create_anchored(path, removal_root, relative, counter, fs)
    }
    /// Create beneath an exclusively prepared root, retaining that descriptor
    /// for cooperative deletion. `relative` contains generated components only.
    pub fn create_anchored(
        path: PathBuf,
        removal_root: Arc<std::fs::File>,
        relative: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> Result<Self, Error> {
        fs.mkdir(&path)?;
        Ok(Self {
            path,
            removal_root,
            relative,
            ledger: Shared::new(Ledger {
                counter,
                claimed: AtomicU64::new(0),
            })?,
            fs,
            removed: false,
        })
    }
    /// Trusted filesystem location, never serialized in a record.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Space still owned, including partially written files.
    #[must_use]
    pub fn claimed(&self) -> u64 {
        self.ledger.claimed.load(Ordering::Acquire)
    }
    fn idle(&self) -> Result<(), Error> {
        if self.removed || Shared::strong_count(&self.ledger) != 1 {
            Err(Error::Internal)
        } else {
            Ok(())
        }
    }
    /// Controller-side relocation, without releasing a single claim.
    pub fn rename(&mut self, to: PathBuf) -> Result<(), Error> {
        self.idle()?;
        let mut root = self.path.as_path();
        for _ in self.relative.components() {
            root = root.parent().ok_or(Error::Internal)?;
        }
        let relative = super::path::join(
            Path::new(""),
            to.strip_prefix(root).map_err(|_| Error::Internal)?,
        )?;
        self.fs.rename(&self.path, &to)?;
        self.path = to;
        self.relative = relative;
        Ok(())
    }
    /// Only a successful deletion releases claims. Can retry after failure.
    pub fn remove(&mut self) -> Result<(), Error> {
        self.idle()?;
        self.fs.remove(&self.path)?;
        self.ledger.counter.release(self.claimed());
        self.ledger.claimed.store(0, Ordering::Release);
        self.removed = true;
        Ok(())
    }
    /// Cooperative no-follow removal. A stop, like a partial I/O failure,
    /// retains the complete claim for the next start.
    pub fn remove_stoppable(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.idle()?;
        self.fs
            .remove_stoppable(&self.removal_root, &self.relative, stopped)?;
        self.ledger.counter.release(self.claimed());
        self.ledger.claimed.store(0, Ordering::Release);
        self.removed = true;
        Ok(())
    }
    fn file(&self, name: &Path) -> Result<SpoolFile, Error> {
        if self.removed {
            return Err(Error::Internal);
        }
        let writer = self.fs.create(&super::path::join(&self.path, name)?)?;
        Ok(SpoolFile {
            writer,
            directory: super::path::join(&self.path, Path::new(""))?,
            ledger: self.ledger.clone(),
            fs: self.fs.clone(),
            length: 0,
            failed: false,
        })
    }
    /// The attempt's sole records file; exclusive create prevents reuse.
    pub fn records(&self) -> Result<SpoolFile, Error> {
        self.file(Path::new("records.ndjson"))
    }
    /// An output named by its stable plan ordinal.
    pub fn output(&self, plan_index: u64) -> Result<SpoolFile, Error> {
        self.file(&super::path::indexed(Path::new(""), "o", plan_index)?)
    }
}

/// Sequential, unbuffered file writes. Claims track rounded cumulative
/// length, while the caller counts work per flush or complete output.
/// An error poisons the writer: no later append/flush is allowed.
pub struct SpoolFile {
    writer: Box<dyn Writer>,
    directory: PathBuf,
    ledger: Shared<Ledger>,
    fs: Arc<dyn FileSystem>,
    length: u64,
    failed: bool,
}
impl SpoolFile {
    /// Predict the directory claim after appending `size` bytes to this
    /// file. Used to encode the final summary's own claim figures.
    pub fn claimed_with_append(&self, size: u64) -> Result<u64, Error> {
        let length = self
            .length
            .checked_add(size)
            .ok_or(Error::Full(Full::SizeBytes))?;
        let delta = self.ledger.counter.increase(self.length, length)?;
        self.ledger
            .claimed
            .load(Ordering::Acquire)
            .checked_add(delta)
            .ok_or(Error::Internal)
    }

    /// Claim before writing. Even a partial write keeps the complete claim
    /// until its directory is successfully deleted.
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.append_stoppable(bytes, &|| false)
    }
    /// Like append, but observe cancellation before claiming and after the
    /// claim, before writing. Cancellation retains every acquired claim.
    pub fn append_stoppable(
        &mut self,
        bytes: &[u8],
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        let result = self.write(bytes, stopped);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn write(&mut self, bytes: &[u8], stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        check_stop(stopped)?;
        let length = self
            .length
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Full(Full::SizeBytes))?;
        let claim = self.ledger.counter.increase(self.length, length)?;
        if self.fs.available(&self.directory)? < self.ledger.counter.reserve {
            return Err(Error::Full(Full::Reserve));
        }
        self.ledger.counter.claim(claim)?;
        self.ledger.claimed.fetch_add(claim, Ordering::AcqRel);
        self.length = length;
        check_stop(stopped)?;
        self.writer.write_all(bytes)?;
        Ok(())
    }
    /// Flush and close. Drop alone never flushes; no sync is requested.
    pub fn finish(self) -> Result<(), Error> {
        self.finish_stoppable(&|| false)
    }
    /// The successful final flush is the seal's commit point.
    pub fn finish_stoppable(mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        check_stop(stopped)?;
        self.writer.flush()?;
        Ok(())
    }
}

fn check_stop(stopped: &dyn Fn() -> bool) -> Result<(), Error> {
    if stopped() {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}
