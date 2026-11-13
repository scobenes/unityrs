//! Pure stored-ZIP layout arithmetic (`BATCH_SPEC` section 8). No payloads,
//! byte emission or digests. B8 must use these same threshold decisions.
use crate::phase::Error;

pub const ZIP16: u64 = 0xffff;
pub const ZIP32: u64 = 0xffff_ffff;
pub const LIMIT: u64 = 10_000 * 16 * 1024 * 1024;
pub const LOCAL: u64 = 30;
pub const CENTRAL: u64 = 46;
pub const END: u64 = 22;
pub const ZIP64_END: u64 = 56 + 20;

fn add(a: u64, b: u64) -> Result<u64, Error> {
    a.checked_add(b).ok_or_else(Error::internal)
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Layout {
    offset: u64,
    central: u64,
    count: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub offset: u64,
    pub local_extra: u64,
    pub central_extra: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct End {
    pub central_offset: u64,
    pub central_size: u64,
    pub count: u64,
    pub zip64: bool,
    pub size: u64,
}
impl End {
    #[must_use]
    pub const fn oversized(self) -> bool {
        self.size > LIMIT
    }
}
impl Layout {
    /// Test-only logical origin and arithmetic seams; never allocate the prefix.
    /// Reject seeds whose complete end cannot be represented.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn seed(offset: u64, central: u64, count: u64) -> Result<Self, Error> {
        let layout = Self {
            offset,
            central,
            count,
        };
        layout.finish()?;
        Ok(layout)
    }

    /// Failed arithmetic leaves the accumulator unchanged.
    pub fn member(&mut self, path_bytes: u64, size: u64) -> Result<Entry, Error> {
        if !(1..=u64::from(u16::MAX)).contains(&path_bytes) {
            return Err(Error::internal());
        }
        let local_extra = if size >= ZIP32 { 20 } else { 0 };
        let fields = u64::from(size >= ZIP32) * 16 + u64::from(self.offset >= ZIP32) * 8;
        let central_extra = if fields == 0 { 0 } else { fields + 4 };
        let offset = add(
            add(add(self.offset, LOCAL)?, path_bytes)?,
            add(local_extra, size)?,
        )?;
        let central = add(self.central, add(add(CENTRAL, path_bytes)?, central_extra)?)?;
        let count = add(self.count, 1)?;
        let entry = Entry {
            offset: self.offset,
            local_extra,
            central_extra,
        };
        *self = Self {
            offset,
            central,
            count,
        };
        Ok(entry)
    }
    pub fn finish(&self) -> Result<End, Error> {
        let zip64 = self.count >= ZIP16 || self.central >= ZIP32 || self.offset >= ZIP32;
        let size = add(
            add(self.offset, self.central)?,
            END + if zip64 { ZIP64_END } else { 0 },
        )?;
        Ok(End {
            central_offset: self.offset,
            central_size: self.central,
            count: self.count,
            zip64,
            size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, ZIP16, ZIP32};
    #[test]
    fn each_end_threshold_is_independent_and_inclusive() {
        for delta in [ZIP32 - 1, ZIP32, ZIP32 + 1] {
            for field in [0, 1] {
                let layout = if field == 0 {
                    Layout {
                        offset: delta,
                        ..Layout::default()
                    }
                } else {
                    Layout {
                        central: delta,
                        ..Layout::default()
                    }
                };
                assert_eq!(layout.finish().unwrap().zip64, delta >= ZIP32);
            }
        }
        for count in [ZIP16 - 1, ZIP16, ZIP16 + 1] {
            assert_eq!(
                Layout {
                    count,
                    ..Layout::default()
                }
                .finish()
                .unwrap()
                .zip64,
                count >= ZIP16
            );
        }
    }
    #[test]
    fn offsets_and_both_size_fields_have_independent_extras() {
        for offset in [ZIP32 - 1, ZIP32, ZIP32 + 1] {
            for size in [ZIP32 - 1, ZIP32, ZIP32 + 1] {
                let mut layout = Layout {
                    offset,
                    ..Layout::default()
                };
                let entry = layout.member(1, size).unwrap();
                assert_eq!(entry.offset, offset);
                assert_eq!(entry.local_extra, if size >= ZIP32 { 20 } else { 0 });
                let fields = 16 * u64::from(size >= ZIP32) + 8 * u64::from(offset >= ZIP32);
                assert_eq!(
                    entry.central_extra,
                    if fields == 0 { 0 } else { fields + 4 }
                );
            }
        }
    }
    #[test]
    fn overflow_is_transactional() {
        for original in [
            Layout {
                offset: u64::MAX,
                ..Layout::default()
            },
            Layout {
                central: u64::MAX,
                ..Layout::default()
            },
            Layout {
                count: u64::MAX,
                ..Layout::default()
            },
        ] {
            let mut layout = original;
            assert!(layout.member(1, 1).is_err());
            assert_eq!(
                (layout.offset, layout.central, layout.count),
                (original.offset, original.central, original.count)
            );
        }
        assert!(
            Layout {
                offset: u64::MAX,
                ..Layout::default()
            }
            .finish()
            .is_err()
        );
        assert!(
            Layout {
                central: u64::MAX,
                ..Layout::default()
            }
            .finish()
            .is_err()
        );
    }
}
