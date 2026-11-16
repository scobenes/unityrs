// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Legacy directory and header (`files/BundleFile.py:395-487`). The version
//! guard is removed for Tier C writing; formats 4+ retain their hash and CRC.
use super::write::{add, aligned};
use super::{BundleFile, BundleHeader, LayoutKind};
use crate::codec::{Codec, encode};
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow, required};
use crate::stream::Endian;
use bytes::Bytes;

impl BundleFile {
    /// Save UnityRaw or UnityWeb's legacy layout, with one level and a
    /// directory padded to four bytes. UnityWeb uses a size-prefixed raw
    /// LZMA1 stream. Format 6 belongs to `save_fs` instead.
    pub fn save_web_raw(&self, options: SaveOptions) -> Result<Bytes, SaveError> {
        self.save_web_raw_impl(options, None, &self.header)
    }

    /// Save legacy layout using caller-owned, metered serialized child snapshots.
    pub fn save_web_raw_with_entries(
        &self,
        options: SaveOptions,
        entries: &[(crate::RawStr, u32, Bytes)],
    ) -> Result<Bytes, SaveError> {
        self.save_web_raw_impl(options, Some(entries), &self.header)
    }

    /// Save a charged legacy directory and current caller-owned header.
    pub fn save_web_raw_with_snapshot(
        &self,
        options: SaveOptions,
        entries: &[(crate::RawStr, u32, Bytes)],
        header: &BundleHeader,
    ) -> Result<Bytes, SaveError> {
        self.save_web_raw_impl(options, Some(entries), header)
    }

    pub(super) fn save_web_raw_impl(
        &self,
        options: SaveOptions,
        entries: Option<&[(crate::RawStr, u32, Bytes)]>,
        header: &BundleHeader,
    ) -> Result<Bytes, SaveError> {
        Self::save_version(header)?;
        if header.layout != LayoutKind::WebRaw {
            return Err(SaveError::Unsupported("legacy layout on an FS bundle"));
        }
        let mut directory_size = 4usize;
        let cabs = if entries.is_some() {
            crate::limits::MeteredVec::new(self.meter())
        } else {
            self.writeable_cabs()?
        };
        for cab in cabs.as_slice() {
            cab.validate_name(options.layout, false)?;
        }
        for slot in self.files.iter().filter(|_| entries.is_none()) {
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
        if let Some(entries) = entries {
            for (name, _, _) in entries {
                self.meter().count_decode(8)?;
                let nul = usize::from(
                    options.layout != SaveLayout::Semantic
                        || name.len() != crate::stream::STRING_TO_NULL_WINDOW,
                );
                directory_size = add(directory_size, add(name.len(), 8 + nul)?)?;
            }
        }
        directory_size = aligned(directory_size, 4)?;
        let mut directory = Writer::metadata(self.meter(), Endian::Big, options.layout)?;
        let mut payload = Writer::new(self.meter(), Endian::Big)?;
        directory.count(entries.map_or_else(
            || add(self.files.len(), cabs.len()),
            |entries| Ok(entries.len()),
        )?)?;
        for slot in self.files.iter().filter(|_| entries.is_none()) {
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
        if let Some(entries) = entries {
            for (name, _, bytes) in entries {
                self.meter().count_decode(8)?;
                directory.cstr(name.as_bytes())?;
                directory.u32(narrow(
                    add(directory_size, payload.len())?,
                    "legacy entry offset",
                )?)?;
                directory.u32(narrow(bytes.len(), "legacy entry size")?)?;
                payload.bytes(bytes)?;
            }
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
        self.save_legacy_content(options, directory_size, directory, header)
    }

    fn save_legacy_content(
        &self,
        options: SaveOptions,
        directory_size: usize,
        directory: Writer,
        header: &BundleHeader,
    ) -> Result<Bytes, SaveError> {
        let uncompressed_size = directory.len();
        let data = if header.signature.as_bytes() == b"UnityWeb" {
            let compressed = encode::registered_compress(
                directory.slice(),
                Codec::Lzma,
                &self.context().codecs,
                self.meter(),
            )?;
            let mut prefixed = Writer::new(self.meter(), Endian::Little)?;
            prefixed.bytes(required(compressed.get(..5), "LZMA properties")?)?;
            prefixed.u64(narrow(uncompressed_size, "LZMA content size")?)?;
            prefixed.bytes(required(compressed.get(5..), "LZMA stream")?)?;
            prefixed.finish()?
        } else {
            directory.finish()?
        };
        let mut out = self.save_header(options, header)?;
        let version = header.format;
        let extra = 24
            + usize::from(version >= 2) * 4
            + usize::from(version >= 3) * 4
            + usize::from(version >= 4) * 20;
        let header_size = aligned(add(out.len(), extra)?, 4)?;
        let total = narrow(add(header_size, data.len())?, "legacy bundle size")?;
        if version >= 4 {
            out.bytes(&required(header.hash, "legacy hash")?)?;
            out.u32(required(header.crc, "legacy CRC")?)?;
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
