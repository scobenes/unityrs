//! An individually owned staged/peek file, with the directory owner's claim
//! rules. No adoption and no release on Drop, including after a partial write.
use super::{Counter, Error, FileSystem, Full, Writer};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct OwnedFile {
    path: PathBuf,
    counter: Arc<Counter>,
    fs: Arc<dyn FileSystem>,
    writer: Option<Box<dyn Writer>>,
    reserved: u64,
    claimed: u64,
    length: u64,
    failed: bool,
    removed: bool,
}
impl OwnedFile {
    /// The parent is prepared. An existing file is never adopted or truncated.
    /// Initially empty; `reserve` makes the known-length claim before copying.
    pub fn create(
        path: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> Result<Self, Error> {
        let writer = fs.create(&path)?;
        Ok(Self {
            path,
            counter,
            fs,
            writer: Some(writer),
            reserved: 0,
            claimed: 0,
            length: 0,
            failed: false,
            removed: false,
        })
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn claimed(&self) -> u64 {
        self.claimed
    }
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.writer.is_none() && !self.failed && !self.removed
    }
    fn writable(&self) -> Result<(), Error> {
        if self.failed || self.removed || self.writer.is_none() {
            Err(Error::Internal)
        } else {
            Ok(())
        }
    }
    /// Reserve a known final length, or grow a previous reservation. The full
    /// rounded claim survives any later failure until explicit deletion.
    pub fn reserve(&mut self, length: u64) -> Result<(), Error> {
        self.writable()?;
        let result = self.claim(length);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn claim(&mut self, length: u64) -> Result<(), Error> {
        let directory = self.path.parent().ok_or(Error::Internal)?;
        if self.fs.available(directory)? < self.counter.reserve {
            return Err(Error::Full(Full::Reserve));
        }
        if length <= self.reserved {
            return Ok(());
        }
        let delta = self.counter.increase(self.reserved, length)?;
        self.counter.claim(delta)?;
        self.claimed += delta;
        self.reserved = length;
        Ok(())
    }
    pub fn append(&mut self, bytes: &[u8], stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.writable()?;
        let result = (|| {
            check_stop(stopped)?;
            let length = self
                .length
                .checked_add(bytes.len() as u64)
                .ok_or(Error::Full(Full::SizeBytes))?;
            self.claim(length)?;
            check_stop(stopped)?;
            self.writer
                .as_mut()
                .ok_or(Error::Internal)?
                .write_all(bytes)?;
            self.length = length;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Flush, make mode `0400` on the open descriptor, then close. A failed
    /// seal is poisoned and still owns every claim. A seal never releases
    /// the unused part of a reservation.
    pub fn seal(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.writable()?;
        let result = (|| {
            check_stop(stopped)?;
            self.writer.as_mut().ok_or(Error::Internal)?.flush()?;
            check_stop(stopped)?;
            self.writer.as_ref().ok_or(Error::Internal)?.seal()?;
            drop(self.writer.take());
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Close any writer and delete. A failed/partial deletion returns nothing;
    /// retry is allowed, but neither successful deletion nor reuse is implicit.
    pub fn remove(&mut self) -> Result<(), Error> {
        if self.removed {
            return Err(Error::Internal);
        }
        drop(self.writer.take());
        self.failed = true;
        self.fs.remove_file(&self.path)?;
        self.counter.release(self.claimed);
        self.claimed = 0;
        self.removed = true;
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
