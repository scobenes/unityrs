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
