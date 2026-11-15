// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/WebFile.py. See THIRD_PARTY_LICENSES.
//! WebFile directory rebuilding (`files/WebFile.py:58-112`). Compression
//! preserves decompressed bytes, with deterministic stored streams. Brotli's
//! metadata marker corrects the oracle writer's undetectable output (Tier C).

use super::{WebFile, WebPacker};
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow};
use crate::files::{BundlePacker, BundleSaveOptions, File};
use crate::limits::MeteredVec;
use crate::stream::Endian;
use bytes::Bytes;

/// WebFile save layout and compression. The default writes plain bytes and
/// `UnityWebData1.0`, as UnityPy does, independently of the input's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebSaveOptions {
    /// Layout to use for nested serialized files and bundles.
    pub layout: SaveLayout,
    /// The enclosing WebFile's compression.
    pub packer: WebPacker,
}

const _: () = crate::send_sync::<WebSaveOptions>();

impl WebFile {
    /// Rebuild the live ordered files dictionary. Duplicate input names retain
    /// their first position and last value. Children use their default packer.
    /// Saving leaves the source bytes and change flags intact.
    pub fn save(&self, options: WebSaveOptions) -> Result<Bytes, SaveError> {
        self.save_with_signature(options, "UnityWebData1.0")
    }

    /// Save with an explicit signature, matching UnityPy's `signature=`.
    /// Semantic layout refuses signatures that the reader cannot round trip.
    pub fn save_with_signature(
        &self,
        options: WebSaveOptions,
        signature: &str,
    ) -> Result<Bytes, SaveError> {
        if options.layout == SaveLayout::Semantic {
            if signature.len() >= crate::stream::STRING_TO_NULL_WINDOW
                || (!signature.starts_with("UnityWebData")
                    && !signature.starts_with("TuanjieWebData"))
            {
                return Err(SaveError::Range("WebFile signature"));
            }
            self.meter().count_decode(signature.len() as u64)?;
            if signature.as_bytes().contains(&0) {
                return Err(SaveError::Range("WebFile signature"));
            }
        }
        let mut header_size = add(signature.len(), 5)?;
        let cabs = self.writeable_cabs()?;
        for cab in cabs.as_slice() {
            cab.validate_name(options.layout, true)?;
        }
        for slot in &self.files {
            self.meter().count_decode(12)?;
            header_size = add(header_size, add(12, slot.name.as_bytes().len())?)?;
        }
        for cab in cabs.as_slice() {
            header_size = add(header_size, add(12, cab.name().len())?)?;
        }
        let mut offset: i32 = narrow(header_size, "WebFile header size")?;
        let mut data = Writer::new(self.meter(), Endian::Little)?;
        let mut sizes =
            MeteredVec::with_capacity(self.meter(), add(self.files.len(), cabs.len())?)?;
        for slot in &self.files {
            let child = match self.entry(slot.dir_index)? {
                File::Raw(raw) => raw.data.clone(),
                File::Serialized(sf) => sf.save(SaveOptions {
                    layout: options.layout,
                })?,
                File::Bundle(bundle) => bundle.save(BundleSaveOptions {
                    layout: options.layout,
                    packer: BundlePacker::None,
                })?,
                File::Web(web) => web.save(WebSaveOptions {
                    layout: options.layout,
                    packer: WebPacker::None,
                })?,
            };
            let size: i32 = narrow(child.len(), "WebFile entry size")?;
            narrow::<i32, _>(
                add(header_size, add(data.len(), child.len())?)?,
                "WebFile size",
            )?;
            sizes.push(size)?;
            data.bytes(&child)?;
        }
        for cab in cabs.as_slice() {
            let child = cab.save()?;
            let size: i32 = narrow(child.len(), "WebFile entry size")?;
            narrow::<i32, _>(
                add(header_size, add(data.len(), child.len())?)?,
                "WebFile size",
            )?;
            sizes.push(size)?;
            data.bytes(&child)?;
        }
        let mut out = Writer::new(self.meter(), Endian::Little)?;
        out.cstr(signature.as_bytes())?;
        out.i32(offset)?;
        for (slot, size) in self.files.iter().zip(sizes.as_slice()) {
            out.i32(offset)?;
            out.i32(*size)?;
            out.i32(narrow(slot.name.as_bytes().len(), "WebFile member name")?)?;
            out.bytes(slot.name.as_bytes())?;
            offset = offset
                .checked_add(*size)
                .ok_or(SaveError::Range("WebFile offset"))?;
        }
        for (cab, size) in cabs
            .as_slice()
            .iter()
            .zip(sizes.as_slice().iter().skip(self.files.len()))
        {
            out.i32(offset)?;
            out.i32(*size)?;
            out.i32(narrow(cab.name().len(), "WebFile member name")?)?;
            out.bytes(cab.name().as_bytes())?;
            offset = offset
                .checked_add(*size)
                .ok_or(SaveError::Range("WebFile offset"))?;
        }
        out.bytes(data.slice())?;
        drop(data);
        if options.layout == SaveLayout::Semantic
            && options.packer == WebPacker::None
            && out.slice().get(32..38) == Some(b"brotli")
        {
            return Err(SaveError::Range("WebFile brotli marker collision"));
        }
        match options.packer {
            WebPacker::None => out.finish(),
            WebPacker::Gzip => gzip(out.slice(), self),
            WebPacker::Brotli => brotli(out.slice(), self),
        }
    }
}

fn add(a: usize, b: usize) -> Result<usize, SaveError> {
    a.checked_add(b).ok_or(SaveError::Range("WebFile size"))
}

// RFC 1951 stored blocks and RFC 1952 framing. No codec state or new heap
// allocation outside Writer; output is charged before every write. UnityPy's
// gzip helper chooses its own clock and compressor (`helpers/CompressionHelper.py:145-155`).
fn gzip(data: &[u8], web: &WebFile) -> Result<Bytes, SaveError> {
    let mut out = Writer::new(web.meter(), Endian::Little)?;
    out.bytes(&[31, 139, 8, 0, 0, 0, 0, 0, 0, 255])?;
    for (i, chunk) in data.chunks(65535).enumerate() {
        out.bytes(&[u8::from(i == (data.len() - 1) / 65535)])?;
        let size: u16 = narrow(chunk.len(), "gzip block size")?;
        out.u16(size)?;
        out.u16(!size)?;
        out.bytes(chunk)?;
    }
    if data.is_empty() {
        out.bytes(&[1, 0, 0, 255, 255])?;
    }
    web.meter().count_decode(data.len() as u64)?;
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    out.u32(!crc)?;
    out.u32(narrow(data.len(), "gzip input size")?)?;
    out.finish()
}

// RFC 7932 sections 9.1 and 9.2: WBITS=16, a skipped metadata block with
// 36 bytes, then nonfinal uncompressed 16-bit-length metablocks, and ISLAST
// with ISLASTEMPTY. Metadata puts Unity's detector marker at byte 32 without
// changing decompressed data. All bit headers fit fixed stack arrays.
fn brotli(data: &[u8], web: &WebFile) -> Result<Bytes, SaveError> {
    let mut out = Writer::new(web.meter(), Endian::Little)?;
    let mut metadata = [0; 38];
    metadata[..2].copy_from_slice(&[0xac, 0x11]);
    metadata[32..].copy_from_slice(b"brotli");
    out.bytes(&metadata)?;
    for chunk in data.chunks(65536) {
        let len: u32 = narrow(chunk.len() - 1, "brotli block size")?;
        let bits = (len << 3) | (1 << 19);
        out.bytes(&bits.to_le_bytes()[..3])?;
        out.bytes(chunk)?;
    }
    out.bytes(&[3])?;
    out.finish()
}
