use super::{Error, Full};
use crate::work::rounded;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared run-wide spool cap. Block size is read once at preflight.
#[derive(Debug)]
pub struct Counter {
    size: u64,
    pub(super) reserve: u64,
    pub(super) block: u64,
    used: AtomicU64,
    peak: AtomicU64,
}
impl Counter {
    /// Reject a zero block size rather than dividing by zero.
    pub fn new(size: u64, reserve: u64, block: u64) -> Result<Arc<Self>, Error> {
        if block == 0 {
            return Err(Error::Internal);
        }
        Ok(Arc::new(Self {
            size,
            reserve,
            block,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }))
    }
    pub(super) fn increase(&self, old: u64, new: u64) -> Result<u64, Error> {
        let before = rounded(old, self.block).ok_or(Error::Full(Full::SizeBytes))?;
        let after = rounded(new, self.block).ok_or(Error::Full(Full::SizeBytes))?;
        after.checked_sub(before).ok_or(Error::Internal)
    }
    pub(super) fn claim(&self, bytes: u64) -> Result<(), Error> {
        let old = self
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&v| v <= self.size)
            })
            .map_err(|_| Error::Full(Full::SizeBytes))?;
        self.peak
            .fetch_max(old.saturating_add(bytes), Ordering::AcqRel);
        Ok(())
    }
    pub(super) fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
    /// Includes claims whose write or later deletion failed.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
    /// Greatest run-wide claim total.
    #[must_use]
    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }
}
