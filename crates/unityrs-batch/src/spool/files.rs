use super::{Counter, Error, FileSystem, Full, output_name};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Ledger {
    counter: Arc<Counter>,
    claimed: AtomicU64,
}
/// Owner of exactly one newly created directory. Keep it through failure
/// cleanup or hand it to the controller; dropping it retains disk claims.
/// It cannot be removed or renamed while a writer still holds it.
pub struct Directory {
    path: PathBuf,
    ledger: Arc<Ledger>,
    fs: Arc<dyn FileSystem>,
    removed: bool,
}
impl Directory {
    /// Parent must already exist; an existing directory is never adopted.
    pub fn create(
        path: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> Result<Self, Error> {
        fs.mkdir(&path)?;
        Ok(Self {
            path,
            ledger: Arc::new(Ledger {
                counter,
                claimed: AtomicU64::new(0),
            }),
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
        if self.removed || Arc::strong_count(&self.ledger) != 1 {
            Err(Error::Internal)
        } else {
            Ok(())
        }
    }
    /// Controller-side relocation, without releasing a single claim.
    pub fn rename(&mut self, to: PathBuf) -> Result<(), Error> {
        self.idle()?;
        self.fs.rename(&self.path, &to)?;
        self.path = to;
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
    fn file(&self, name: &Path) -> Result<SpoolFile, Error> {
        if self.removed {
            return Err(Error::Internal);
        }
        let writer = self.fs.create(&self.path.join(name))?;
        Ok(SpoolFile {
            writer,
            directory: self.path.clone(),
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
        self.file(&output_name(plan_index))
    }
}

/// Sequential, unbuffered file writes. Claims track rounded cumulative
/// length, while the caller counts work per flush or complete output.
/// An error poisons the writer: no later append/flush is allowed.
pub struct SpoolFile {
    writer: Box<dyn Write + Send>,
    directory: PathBuf,
    ledger: Arc<Ledger>,
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
        if self.failed {
            return Err(Error::Internal);
        }
        let result = self.write(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
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
        self.writer.write_all(bytes)?;
        Ok(())
    }
    /// Flush and close. Drop alone never flushes; no sync is requested.
    pub fn finish(mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        self.writer.flush()?;
        Ok(())
    }
}
