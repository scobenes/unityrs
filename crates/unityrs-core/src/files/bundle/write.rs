// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Bundle save and block/directory layout (`files/BundleFile.py:188-393`).
//! Output and all temporary buffers charge the opened input's meter.

use super::{BundleFile, LayoutKind};
use crate::codec::{Codec, encode};
use crate::files::File;
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow, required};
use crate::stream::Endian;
use crate::version::FlagsClass;
use bytes::Bytes;

/// Bundle packer. Tuple flags follow the code's order, not its docstring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BundlePacker {
    /// Store blocks and blocks-info with flags 0x40.
    #[default]
    None,
    /// Keep the header flags and the first block's flags for every block.
    Original,
    /// UnityPy 1.25.2's (194, 2): HC level 9, 128 KiB chunks.
    Lz4,
    /// Raw LZMA1 with UnityPy's settings, (65, 1).
    Lzma,
    /// HEAD 417d595's HC level 9 packer and cumulative-output chunk rule.
    /// For 1.25.2 HC use `Flags { data: 195, blocks: 3 }`.
    Lz4Hc,
    /// `(data_flag, block_info_flag)` as `save_fs` actually takes them.
    Flags {
        /// Header layout bits and blocks-info codec.
        data: u32,
        /// Data blocks' flags and codec.
        blocks: u16,
    },
}

/// Bundle save layout and compression. Children use the same layout and
/// their default packer, as UnityPy calls their `save()` with no arguments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BundleSaveOptions {
    /// Metadata layout, including the held serialized files' layout.
    pub layout: SaveLayout,
    /// The enclosing bundle's packer.
    pub packer: BundlePacker,
}

const _: () = crate::send_sync::<BundlePacker>();
const _: () = crate::send_sync::<BundleSaveOptions>();

impl BundleFile {
    /// Rebuild the bundle from its live ordered `files` dictionary.
    /// Repeated names collapse to their final row at the first position.
    /// Saving retains change flags and leaves the original input intact.
    /// `None` provides canonical idempotence. `Original` adopts the first
    /// loaded block's codec, so a stored first chunk can change the next
    /// `Original` save's block layout, as in UnityPy.
    ///
    /// Format 9 and named HC follow HEAD 417d595. Earlier formats retain
    /// 1.25.2's other packers. Formats above 9 refuse before output charges.
    /// Legacy signatures ignore the packer, as UnityPy does.
    pub fn save(&self, options: BundleSaveOptions) -> Result<Bytes, SaveError> {
        self.save_impl(options, None)
    }

    /// Save an ordered caller-supplied directory of already serialized children.
    /// The caller owns and meters the names and payload snapshots.
    pub fn save_with_entries(
        &self,
        options: BundleSaveOptions,
        entries: &[(crate::RawStr, u32, Bytes)],
    ) -> Result<Bytes, SaveError> {
        self.save_impl(options, Some(entries))
    }

    fn save_impl(
        &self,
        options: BundleSaveOptions,
        entries: Option<&[(crate::RawStr, u32, Bytes)]>,
    ) -> Result<Bytes, SaveError> {
        self.save_version()?;
        let child_options = SaveOptions {
            layout: options.layout,
        };
        if self.header.layout == LayoutKind::WebRaw {
            return self.save_web_raw_impl(child_options, entries);
        }
        let (data, blocks) = if self.header.signature.as_bytes() == b"UnityFS" {
            match options.packer {
                BundlePacker::None => (64, 64),
                BundlePacker::Original => {
                    let flags = required(self.header.data_flags, "data flags")?;
                    // Tier C writing: an empty bundle has no first block.
                    let blocks = self.blocks.first().map_or(flags, |b| b.flags);
                    (flags, narrow(blocks, "block flags")?)
                }
                BundlePacker::Lz4 => (194, 2),
                BundlePacker::Lzma => (65, 1),
                BundlePacker::Lz4Hc => (195, 3),
                BundlePacker::Flags { data, blocks } => (data, blocks),
            }
        } else {
            (64, 64)
        };
        self.save_fs_impl(
            data,
            blocks,
            child_options,
            options.packer == BundlePacker::Lz4Hc,
            entries,
        )
    }

    /// Save the FS layout, with header/blocks-info flags first and data
    /// block flags second. Encryption is removed, never emitted: exactly
    /// the encryption mask's bits and each block's 0x100 bit are cleared.
    /// This Tier C correction applies in both layout modes.
    pub fn save_fs(
        &self,
        data_flag: u32,
        block_info_flag: u16,
        options: SaveOptions,
    ) -> Result<Bytes, SaveError> {
        self.save_fs_impl(data_flag, block_info_flag, options, false, None)
    }

    /// Save the FS body using an ordered supplied child directory.
    pub fn save_fs_with_entries(
        &self,
        data_flag: u32,
        block_info_flag: u16,
        options: SaveOptions,
        entries: &[(crate::RawStr, u32, Bytes)],
    ) -> Result<Bytes, SaveError> {
        self.save_fs_impl(data_flag, block_info_flag, options, false, Some(entries))
    }

    fn save_fs_impl(
        &self,
        data_flag: u32,
        block_info_flag: u16,
        options: SaveOptions,
        named_hc: bool,
        entries: Option<&[(crate::RawStr, u32, Bytes)]>,
    ) -> Result<Bytes, SaveError> {
        self.save_version()?;
        if self.header.layout != LayoutKind::Fs {
            return Err(SaveError::Unsupported("FS layout on a legacy bundle"));
        }
        let mask = match required(self.header.flags_class, "flags class")? {
            FlagsClass::New => 0x1400,
            FlagsClass::Old => 0x200,
        };
        let data_flag = data_flag & !mask;
        let block_info_flag =
            block_info_flag & !(narrow::<u16, _>(mask, "encryption mask")? | 0x100);
        if data_flag & 0x40 == 0 {
            return Err(SaveError::Missing("directory flag 0x40"));
        }
        let (file_data, directory) = if let Some(entries) = entries {
            self.save_supplied_entries(options, entries)?
        } else {
            self.save_entries(options)?
        };
        let packed = if named_hc || self.header.format >= 9 {
            encode::head_chunk_based_compress(
                &file_data,
                block_info_flag,
                self.header.format,
                self.meter(),
            )?
        } else {
            encode::chunk_based_compress(&file_data, block_info_flag, self.meter())?
        };
        let mut info = Writer::new(self.meter(), Endian::Big)?;
        info.bytes(&[0; 16])?;
        info.count(packed.blocks.len())?;
        for block in packed.blocks.as_slice() {
            info.u32(block.uncompressed)?;
            info.u32(block.compressed)?;
            info.u16(block.flags)?;
            if let Some(offset) = block.offset {
                info.u64(offset)?;
            }
        }
        info.bytes(&directory)?;
        let info_len = narrow(info.len(), "blocks-info size")?;
        let compressed =
            encode::compress(info.slice(), Codec::from_flags(data_flag), self.meter())?;
        let mut out = self.save_header(options)?;
        let extra = usize::from(self.header.signature.as_bytes() != b"UnityFS");
        let mut header_size = add(out.len(), 20 + extra)?;
        if self.header.block_alignment {
            header_size = aligned(header_size, 16)?;
        }
        let body_size = if data_flag & 0x80 != 0 {
            let start = if data_flag & 0x200 != 0 {
                aligned(header_size, 16)?
            } else {
                header_size
            };
            add(add(start, packed.data.len())?, compressed.len())?
        } else {
            let start = add(header_size, compressed.len())?;
            let start = if data_flag & 0x200 != 0 {
                aligned(start, 16)?
            } else {
                start
            };
            add(start, packed.data.len())?
        };
        out.i64(narrow(body_size, "bundle size")?)?;
        out.u32(narrow(compressed.len(), "compressed blocks-info size")?)?;
        out.u32(info_len)?;
        out.u32(data_flag)?;
        if extra != 0 {
            out.bytes(&[0])?;
        }
        if self.header.block_alignment {
            out.align(16)?;
        }
        if data_flag & 0x80 != 0 {
            if data_flag & 0x200 != 0 {
                out.align(16)?;
            }
            out.bytes(&packed.data)?;
            out.bytes(&compressed)?;
        } else {
            out.bytes(&compressed)?;
            if data_flag & 0x200 != 0 {
                out.align(16)?;
            }
            out.bytes(&packed.data)?;
        }
        out.finish()
    }

    fn save_supplied_entries(
        &self,
        options: SaveOptions,
        entries: &[(crate::RawStr, u32, Bytes)],
    ) -> Result<(Bytes, Bytes), SaveError> {
        let mut data = Writer::new(self.meter(), Endian::Big)?;
        let mut directory = Writer::metadata(self.meter(), Endian::Big, options.layout)?;
        directory.count(entries.len())?;
        for (name, flags, bytes) in entries {
            self.meter().count_decode(8)?;
            directory.i64(narrow(data.len(), "entry offset")?)?;
            directory.i64(narrow(bytes.len(), "entry size")?)?;
            directory.u32(*flags)?;
            directory.cstr(name.as_bytes())?;
            data.bytes(bytes)?;
        }
        directory.end_metadata()?;
        Ok((data.finish()?, directory.finish()?))
    }

    pub(super) fn save_version(&self) -> Result<(), SaveError> {
        if self.header.format > 9 {
            return Err(SaveError::HeadTierWritesNotImplemented);
        }
        Ok(())
    }

    pub(super) fn save_header(&self, options: SaveOptions) -> Result<Writer, SaveError> {
        let mut out = Writer::metadata(self.meter(), Endian::Big, options.layout)?;
        out.cstr(self.header.signature.as_bytes())?;
        out.u32(self.header.format)?;
        out.cstr(self.header.version_player.as_bytes())?;
        out.cstr(self.header.version_engine.as_bytes())?;
        Ok(out)
    }

    fn save_entries(&self, options: SaveOptions) -> Result<(Bytes, Bytes), SaveError> {
        let cabs = self.writeable_cabs()?;
        for cab in cabs.as_slice() {
            cab.validate_name(options.layout, false)?;
        }
        let mut data = Writer::new(self.meter(), Endian::Big)?;
        let mut directory = Writer::metadata(self.meter(), Endian::Big, options.layout)?;
        directory.count(add(self.files.len(), cabs.len())?)?;
        for slot in &self.files {
            self.meter().count_decode(8)?;
            let row = required(self.directory.get(slot.dir_index), "directory row")?;
            let bytes = self.save_child(slot.dir_index, options)?;
            directory.i64(narrow(data.len(), "entry offset")?)?;
            directory.i64(narrow(bytes.len(), "entry size")?)?;
            directory.u32(row.flags.unwrap_or(0))?;
            directory.cstr(slot.name.as_bytes())?;
            data.bytes(&bytes)?;
        }
        for cab in cabs.as_slice() {
            let bytes = cab.save()?;
            directory.i64(narrow(data.len(), "entry offset")?)?;
            directory.i64(narrow(bytes.len(), "entry size")?)?;
            directory.u32(cab.flags())?;
            directory.cstr(cab.name().as_bytes())?;
            data.bytes(&bytes)?;
        }
        directory.end_metadata()?;
        Ok((data.finish()?, directory.finish()?))
    }

    pub(super) fn save_child(
        &self,
        index: usize,
        options: SaveOptions,
    ) -> Result<Bytes, SaveError> {
        match self.entry(index)? {
            File::Raw(raw) => Ok(raw.data.clone()),
            File::Serialized(sf) => sf.save(options),
            File::Bundle(bundle) => bundle.save(BundleSaveOptions {
                layout: options.layout,
                packer: BundlePacker::None,
            }),
            File::Web(web) => web.save(crate::files::WebSaveOptions {
                layout: options.layout,
                packer: crate::files::WebPacker::None,
            }),
        }
    }
}

pub(super) fn add(a: usize, b: usize) -> Result<usize, SaveError> {
    a.checked_add(b).ok_or(SaveError::Range("bundle size"))
}

pub(super) fn aligned(n: usize, alignment: usize) -> Result<usize, SaveError> {
    add(n, (alignment - n % alignment) % alignment)
}
