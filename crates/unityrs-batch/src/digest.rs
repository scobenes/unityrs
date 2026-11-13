//! Output checksums and the 16 MiB multipart `ETag`; bounded stack state.
use md5::{Digest, Md5};
use std::fmt;

/// Multipart upload/verification part size.
pub const PART: usize = 16 * 1024 * 1024;
/// Lowercase hex without an intermediate string.
pub struct Hex<'a>(pub &'a [u8]);
impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
/// Quotes removed, as stored in the manifest. An exact multiple of PART
/// has no empty final part. A single-part object uses its ordinary MD5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Etag {
    digest: [u8; 16],
    parts: Option<u64>,
}
impl Etag {
    /// Whether records carry this `ETag` rather than null.
    #[must_use]
    pub fn is_multipart(self) -> bool {
        self.parts.is_some()
    }
}
impl fmt::Display for Etag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Hex(&self.digest))?;
        if let Some(n) = self.parts {
            write!(f, "-{n}")?;
        }
        Ok(())
    }
}
/// Hashes of the content, with no retained bytes or heap allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hashes {
    /// Whole-file MD5.
    pub md5: [u8; 16],
    /// Standard reflected CRC-32 (ZIP/gzip).
    pub crc32: u32,
    /// Ordinary MD5 up to PART; multipart `ETag` strictly above PART.
    pub etag: Etag,
}
impl Hashes {
    /// Borrow already charged output memory; do not copy it.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let md5 = Md5::digest(bytes).into();
        let mut etag = Etag {
            digest: md5,
            parts: None,
        };
        if bytes.len() > PART {
            let mut combined = Md5::new();
            let mut count = 0u64;
            for part in bytes.chunks(PART) {
                combined.update(Md5::digest(part));
                count += 1;
            }
            etag = Etag {
                digest: combined.finalize().into(),
                parts: Some(count),
            };
        }
        Self {
            md5,
            crc32: crc32fast::hash(bytes),
            etag,
        }
    }
}

/// Incremental archive hashes. Three MD5 states, no retained part-digest list.
/// Empty input and exactly one part use the whole-file MD5 as their `ETag`.
#[derive(Default)]
pub struct Incremental {
    whole: Md5,
    current: Md5,
    combined: Md5,
    size: u64,
    part_size: usize,
    parts: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamHashes {
    pub size: u64,
    pub md5: [u8; 16],
    pub etag: Etag,
}
impl Incremental {
    pub fn update(&mut self, mut bytes: &[u8]) -> Result<(), crate::phase::Error> {
        let size = self
            .size
            .checked_add(u64::try_from(bytes.len()).map_err(|_| crate::phase::Error::internal())?)
            .ok_or_else(crate::phase::Error::internal)?;
        self.whole.update(bytes);
        while !bytes.is_empty() {
            let n = bytes.len().min(PART - self.part_size);
            let (part, rest) = bytes
                .split_at_checked(n)
                .ok_or_else(crate::phase::Error::internal)?;
            self.current.update(part);
            self.part_size += n;
            bytes = rest;
            if self.part_size == PART {
                self.combined
                    .update(std::mem::take(&mut self.current).finalize());
                // At most u64::MAX / PART completed parts can exist: size was checked above.
                self.parts += 1;
                self.part_size = 0;
            }
        }
        self.size = size;
        Ok(())
    }
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
    #[must_use]
    pub fn finish(mut self) -> StreamHashes {
        let md5 = self.whole.finalize().into();
        let etag = if self.size <= 16 * 1024 * 1024 {
            Etag {
                digest: md5,
                parts: None,
            }
        } else {
            if self.part_size > 0 {
                self.combined.update(self.current.finalize());
                self.parts += 1;
            }
            Etag {
                digest: self.combined.finalize().into(),
                parts: Some(self.parts),
            }
        };
        StreamHashes {
            size: self.size,
            md5,
            etag,
        }
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::{Incremental, PART};
    #[test]
    fn arithmetic_near_ten_thousand_parts_adds_no_empty_part() {
        // State seam checks counts only; it does not claim to hash 156.25 GiB.
        for parts in [9_999, 10_000, 10_001] {
            let digest = Incremental {
                size: parts * u64::try_from(PART).unwrap(),
                parts,
                ..Incremental::default()
            };
            assert_eq!(digest.finish().etag.parts, Some(parts));
            let mut digest = Incremental {
                size: parts * u64::try_from(PART).unwrap(),
                parts,
                ..Incremental::default()
            };
            digest.update(b"x").unwrap();
            assert_eq!(digest.finish().etag.parts, Some(parts + 1));
        }
    }
    #[test]
    fn byte_count_overflow_is_rejected_before_hashing() {
        let mut digest = Incremental {
            size: u64::MAX,
            ..Incremental::default()
        };
        assert_eq!(digest.update(b"x"), Err(crate::phase::Error::internal()));
        assert_eq!(digest.size(), u64::MAX);
    }
}
