//! Deterministic export work, distinct from physical spool claims.
use crate::reason::{Limit, LimitKind};

/// Overflow-safe rounding, including unusual filesystem block sizes.
/// Zero length claims zero disk blocks; work applies its own floor.
#[must_use]
pub fn rounded(size: u64, block: u64) -> Option<u64> {
    if block == 0 {
        return None;
    }
    let remainder = size % block;
    if remainder == 0 {
        Some(size)
    } else {
        size.checked_add(block - remainder)
    }
}

/// One attempt's work, including the increment that first exceeded its cap.
#[derive(Debug)]
pub struct Work {
    limit: u64,
    export: u64,
    spooled: u64,
}
impl Work {
    /// Spec's `max_export_work`.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            export: 0,
            spooled: 0,
        }
    }
    fn error(&self) -> Limit {
        Limit {
            kind: LimitKind::ExportWork,
            value: self.limit,
        }
    }
    fn add(&mut self, bytes: u64, spool: bool) -> Result<(), Limit> {
        let export = self.export.checked_add(bytes);
        self.export = export.unwrap_or(u64::MAX);
        if spool {
            self.spooled = self.spooled.saturating_add(bytes);
        }
        if export.is_none() || self.export > self.limit {
            Err(self.error())
        } else {
            Ok(())
        }
    }
    /// Count before charging/filling a pixel buffer.
    pub fn pixels(&mut self, bytes: u64) -> Result<(), Limit> {
        self.add(bytes, false)
    }
    /// Each output file or actual records flush, before its disk claim.
    pub fn file(&mut self, size: u64) -> Result<(), Limit> {
        let Some(n) = rounded(size, 4096) else {
            self.export = u64::MAX;
            self.spooled = u64::MAX;
            return Err(self.error());
        };
        self.add(n.max(4096), true)
    }
    /// Largest final `archive_members` encoding, including newline, archive
    /// key and left-out reason; no disk-block rounding.
    pub fn member(&mut self, largest_row: u64) -> Result<(), Limit> {
        self.add(largest_row, true)
    }
    /// A separately written failure record counts only toward spooled part.
    pub fn failure_record(&mut self, size: u64) -> Result<(), Limit> {
        let n = rounded(size, 4096).ok_or_else(|| self.error())?.max(4096);
        self.spooled = self.spooled.checked_add(n).ok_or_else(|| self.error())?;
        Ok(())
    }
    /// Work so far, including refused increments.
    #[must_use]
    pub fn export_work(&self) -> u64 {
        self.export
    }
    /// Work's non-pixel part plus the controller's failure record.
    #[must_use]
    pub fn spooled_part(&self) -> u64 {
        self.spooled
    }
}
