// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Legacy directory and header (`files/BundleFile.py:395-487`). The version
//! guard is removed for Tier C writing; formats 4+ retain their hash and CRC.
use super::write::{add, aligned};
use super::{BundleFile, LayoutKind};
use crate::codec::{Codec, encode};
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow, required};
use crate::stream::Endian;
use bytes::Bytes;

impl BundleFile {
    /// Save UnityRaw or UnityWeb's legacy layout, with one level and a
    /// directory padded to four bytes. UnityWeb uses a size-prefixed raw
    /// LZMA1 stream. Format 6 belongs to `save_fs` instead.
    pub fn save_web_raw(&self, options: SaveOptions) -> Result<Bytes, SaveError> {
        self.save_version()?;
        if self.header.layout != LayoutKind::WebRaw {
            return Err(SaveError::Unsupported("legacy layout on an FS bundle"));
        }
        let mut directory_size = 4usize;
        let cabs = self.writeable_cabs()?;
        for cab in cabs.as_slice() {
            cab.validate_name(options.layout, false)?;
        }
        for slot in &self.files {
            self.meter().count_decode(8)?;
            let nul = usize::from(
                options.layout != SaveLayout::Semantic
                    || slot.name.len() != crate::stream::STRING_TO_NULL_WINDOW,
            );
            directory_size = add(directory_size, add(slot.name.len(), 8 + nul)?)?;
        }
        for cab in cabs.as_slice() {
            let nul = usize::from(
                options.layout != SaveLayout::Semantic
                    || cab.name().len() != crate::stream::STRING_TO_NULL_WINDOW,
            );
            directory_size = add(directory_size, add(cab.name().len(), 8 + nul)?)?;
        }
        directory_size = aligned(directory_size, 4)?;
        let mut directory = Writer::metadata(self.meter(), Endian::Big, options.layout)?;
        let mut payload = Writer::new(self.meter(), Endian::Big)?;
        directory.count(add(self.files.len(), cabs.len())?)?;
        for slot in &self.files {
            self.meter().count_decode(8)?;
            let bytes = self.save_child(slot.dir_index, options)?;
            directory.cstr(slot.name.as_bytes())?;
            directory.u32(narrow(
                add(directory_size, payload.len())?,
                "legacy entry offset",
            )?)?;
            directory.u32(narrow(bytes.len(), "legacy entry size")?)?;
            payload.bytes(&bytes)?;
        }
        for cab in cabs.as_slice() {
            let bytes = cab.save()?;
            directory.cstr(cab.name().as_bytes())?;
            directory.u32(narrow(
                add(directory_size, payload.len())?,
                "legacy entry offset",
            )?)?;
            directory.u32(narrow(bytes.len(), "legacy entry size")?)?;
            payload.bytes(&bytes)?;
        }
        directory.align(4)?;
        // A full-window last name is followed by its numeric fields, so
        // there is no unterminated string at the end of this directory.
        directory.bytes(payload.slice())?;
        drop(payload);
        let uncompressed_size = directory.len();
        let data = if self.header.signature.as_bytes() == b"UnityWeb" {
            let compressed = encode::compress(directory.slice(), Codec::Lzma, self.meter())?;
            let mut prefixed = Writer::new(self.meter(), Endian::Little)?;
            prefixed.bytes(required(compressed.get(..5), "LZMA properties")?)?;
            prefixed.u64(narrow(uncompressed_size, "LZMA content size")?)?;
            prefixed.bytes(required(compressed.get(5..), "LZMA stream")?)?;
            prefixed.finish()?
        } else {
            directory.finish()?
        };
        let mut out = self.save_header(options)?;
        let version = self.header.format;
        let extra = 24
            + usize::from(version >= 2) * 4
            + usize::from(version >= 3) * 4
            + usize::from(version >= 4) * 20;
        let header_size = aligned(add(out.len(), extra)?, 4)?;
        let total = narrow(add(header_size, data.len())?, "legacy bundle size")?;
        if version >= 4 {
            out.bytes(&required(self.header.hash, "legacy hash")?)?;
            out.u32(required(self.header.crc, "legacy CRC")?)?;
        }
        out.u32(total)?;
        out.u32(narrow(header_size, "legacy header size")?)?;
        out.u32(1)?;
        out.i32(1)?;
        out.u32(narrow(data.len(), "legacy compressed size")?)?;
        out.u32(narrow(uncompressed_size, "legacy uncompressed size")?)?;
        if version >= 2 {
            out.u32(total)?;
        }
        if version >= 3 {
            out.u32(narrow(directory_size, "legacy directory size")?)?;
        }
        out.align(4)?;
        out.bytes(&data)?;
        out.finish()
    }
}
