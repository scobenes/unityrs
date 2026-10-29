// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! [`BundleFile::open`]: the header, blocks-info and directory, read as
//! `read_fs` reads them, UnityPy's `files` dict, and where each block's
//! stored bytes are. No data block is decoded.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use super::decode::decode_one;
use super::{BlockInfo, BundleFile, BundleHeader, DirEntry, FileSlot, LayoutKind};
use crate::codec::Codec;
use crate::error::{Error, Unsupported};
use crate::files::{File, Parent};
use crate::limits::{Charge, Meter, MeteredVec, heap_cost};
use crate::once::ResultCell;
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, Reader, py_read};
use crate::typetree::parse::string_to_null;
use crate::version::{
    FlagsClass, ResolvedVersion, flags_class, resolve_bundle, uses_block_alignment,
};

impl BundleFile {
    /// Reads the header, blocks-info and directory. Decodes no data block.
    /// Everything it keeps is charged to `meter`, the input's.
    pub fn open(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        let mut charge = meter.charge(heap_cost(
            u64::try_from(std::mem::size_of::<Self>() + 16).unwrap_or(u64::MAX),
        ))?;
        let mut r = Reader::new(&data, Endian::Big);
        let FsHeader {
            signature,
            format,
            version_player,
            version_engine,
            info_compressed,
            info_uncompressed,
            data_flags,
            engine,
            class,
            encrypted,
            block_alignment,
        } = read_fs_header(&mut r, &data, ctx)?;
        charge.grow(u64::try_from(engine.version.heap_bytes()).unwrap_or(u64::MAX))?;
        // :133-139
        let start = r.position();
        let info_range = if data_flags & 0x80 != 0 {
            let len = i128::try_from(data.len()).unwrap_or(i128::MAX);
            py_read(
                data.len(),
                len - i128::from(info_compressed),
                i64::from(info_compressed),
            )
            .0
        } else {
            let (range, pos) = py_read(data.len(), start, i64::from(info_compressed));
            r.set_position(pos);
            range
        };
        let info_read = u32::try_from(info_range.len()).unwrap_or(u32::MAX);
        let blocks_info = BlockInfo {
            uncompressed: info_uncompressed,
            compressed: info_compressed,
            bytes_read: info_read,
            flags: data_flags,
        };
        let info = decode_one(
            ctx,
            meter,
            data.slice(info_range),
            info_uncompressed,
            data_flags,
            0,
        )?;
        let (blocks, directory) = read_blocks_info(&info, meter)?;
        // :165-166
        if class == FlagsClass::New && data_flags & 0x200 != 0 {
            r.align(16);
        }
        // :168-178: the blocks, read in order from here.
        let mut blocks = blocks;
        let (stored, lengths) = stored_blocks(&mut blocks, &mut r, meter)?;
        let files = files_dict(&directory, meter)?;
        let mut entries: MeteredVec<ResultCell<File, Error>> = MeteredVec::new(meter);
        for _ in 0..directory.len() {
            entries.push(ResultCell::new())?;
        }
        let header = BundleHeader {
            signature,
            format,
            version_player,
            version_engine,
            layout: LayoutKind::Fs,
            engine_version: Some(Ok(engine)),
            data_flags: Some(data_flags),
            flags_class: Some(class),
            block_alignment,
            encrypted,
            hash: None,
            crc: None,
            blocks_info: Some(blocks_info),
        };
        let blocks = keep(blocks, &mut charge)?;
        let directory = keep(directory, &mut charge)?;
        let files = keep(files, &mut charge)?;
        let lengths = keep(lengths, &mut charge)?;
        let entries = keep(entries, &mut charge)?;
        let stored = keep(stored, &mut charge)?;
        Ok(Arc::new_cyclic(|this| Self {
            name,
            source: data,
            header,
            blocks,
            directory,
            files,
            parent,
            lengths,
            runs: Mutex::new(MeteredVec::new(meter)),
            entries,
            charge,
            ctx: ctx.clone(),
            stored,
            this: this.clone(),
        }))
    }
}

/// Where each block's stored bytes are (`reader.read_bytes(compressedSize)`
/// in order from `r`), its `bytes_read` set; and the length cells, a stored
/// block's filled with what was read (`lambda cd, _ucs: cd`).
#[allow(clippy::type_complexity)]
fn stored_blocks(
    blocks: &mut MeteredVec<BlockInfo>,
    r: &mut Reader<'_>,
    meter: &Arc<Meter>,
) -> Result<(MeteredVec<Range<usize>>, MeteredVec<ResultCell<u64, Error>>), Error> {
    let len = r.data().len();
    let mut stored = MeteredVec::new(meter);
    let mut lengths = MeteredVec::new(meter);
    for b in blocks.as_mut_slice() {
        let (range, pos) = py_read(len, r.position(), i64::from(b.compressed));
        r.set_position(pos);
        b.bytes_read = u32::try_from(range.len()).unwrap_or(u32::MAX);
        stored.push(range)?;
        let cell = ResultCell::new();
        if Codec::from_flags(u32::from(b.flags_u16())) == Codec::None {
            let _ = cell.set(Ok(u64::from(b.bytes_read)));
        }
        lengths.push(cell)?;
    }
    Ok((stored, lengths))
}

/// The header fields `read_fs` reads before the blocks-info section.
struct FsHeader {
    signature: RawStr,
    format: u32,
    version_player: RawStr,
    version_engine: RawStr,
    info_compressed: u32,
    info_uncompressed: u32,
    data_flags: u32,
    engine: ResolvedVersion,
    class: FlagsClass,
    encrypted: bool,
    block_alignment: bool,
}

/// `BundleFile.__init__` up to the signature dispatch, then `read_fs` up to
/// the blocks-info section (`BundleFile.py:36-51`, `:93-131`); leaves `r` where
/// blocks-info would start.
fn read_fs_header(r: &mut Reader<'_>, data: &Bytes, ctx: &FileContext) -> Result<FsHeader, Error> {
    // UnityPy files/BundleFile.py:36-39
    let signature = string_to_null(r, data)?;
    let format = r.read_u32()?;
    let version_player = string_to_null(r, data)?;
    let version_engine = string_to_null(r, data)?;
    // :41-51
    match signature.as_bytes() {
        b"UnityArchive" => return Err(Error::Unsupported(Unsupported::UnityArchive)),
        b"UnityWeb" | b"UnityRaw" if format != 6 => {
            return Err(Error::Unsupported(Unsupported::WebRawLayout));
        }
        b"UnityWeb" | b"UnityRaw" | b"UnityFS" => {}
        _ => {
            return Err(Error::Malformed {
                what: "bundle signature",
                detail: "Unknown Bundle signature".into(),
            });
        }
    }
    // :94-105
    let _size = r.read_i64()?;
    let info_compressed = r.read_u32()?;
    let info_uncompressed = r.read_u32()?;
    let data_flags = r.read_u32()?;
    if signature.as_bytes() != b"UnityFS" {
        r.read_i8()?;
    }
    // :107-121
    let engine = resolve_bundle(&version_engine, &ctx.version)?;
    let class = flags_class(&engine.version)?;
    // :123-124: the encryption flag, before the alignment.
    let mask = match class {
        FlagsClass::Old => 0x200,
        FlagsClass::New => 0x1400,
    };
    let encrypted = data_flags & mask != 0;
    if encrypted {
        if ctx.decryptor.is_none() {
            return Err(Error::Unsupported(Unsupported::Encrypted));
        }
        // `ArchiveStorageDecryptor(reader)`: a u32, then two vectors of
        // 16 + 16 bytes and a skipped byte (UnityCN: P3).
        r.read_u32()?;
        for _ in 0..2 {
            r.read_bytes_short(32);
            r.set_position(r.position() + 1);
        }
    }
    // :129-131
    let block_alignment = uses_block_alignment(format, &engine.version)?;
    if block_alignment {
        r.align(16);
    }
    Ok(FsHeader {
        signature,
        format,
        version_player,
        version_engine,
        info_compressed,
        info_uncompressed,
        data_flags,
        engine,
        class,
        encrypted,
        block_alignment,
    })
}

/// The blocks-info and directory tables (`BundleFile.py:141-162`), read
/// big-endian from the decoded section; paths are slices of it.
fn read_blocks_info(
    info: &Bytes,
    meter: &Arc<Meter>,
) -> Result<(MeteredVec<BlockInfo>, MeteredVec<DirEntry>), Error> {
    let mut r = Reader::new(info, Endian::Big);
    // The uncompressed-data hash, read and discarded (short at the end).
    r.read_bytes_short(16);
    let mut blocks = MeteredVec::new(meter);
    for _ in 0..r.read_i32()?.max(0) {
        let uncompressed = r.read_u32()?;
        let compressed = r.read_u32()?;
        let flags = u32::from(r.read_u16()?);
        blocks.push(BlockInfo {
            uncompressed,
            compressed,
            bytes_read: 0,
            flags,
        })?;
    }
    let mut directory = MeteredVec::new(meter);
    for _ in 0..r.read_i32()?.max(0) {
        let offset = r.read_i64()?;
        let size = r.read_i64()?;
        let flags = r.read_u32()?;
        let path = string_to_null(&mut r, info)?;
        directory.push(DirEntry {
            offset,
            size,
            flags: Some(flags),
            path,
        })?;
    }
    Ok((blocks, directory))
}

/// UnityPy's `files` dict over the directory: each path at its first
/// position, holding its last entry. Computed by sorting.
fn files_dict(
    dir: &MeteredVec<DirEntry>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<FileSlot>, Error> {
    let rows = dir.as_slice();
    let mut perm: MeteredVec<usize> = MeteredVec::with_capacity(meter, rows.len())?;
    for i in 0..rows.len() {
        perm.push(i)?;
    }
    let path = |i: &usize| rows.get(*i).map(|d| d.path.as_bytes()).unwrap_or_default();
    // Stable by the index tiebreak, and in place: a stable sort's scratch
    // (half the rows or more) would be heap no meter charged (plan 3.15).
    perm.as_mut_slice()
        .sort_unstable_by(|a, b| path(a).cmp(path(b)).then(a.cmp(b)));
    let mut groups: MeteredVec<(usize, usize)> = MeteredVec::new(meter);
    let all = perm.as_slice();
    let mut at = 0;
    while let Some(first) = all.get(at) {
        let mut last = *first;
        let mut end = at + 1;
        while let Some(next) = all.get(end).filter(|n| path(n) == path(first)) {
            last = *next;
            end += 1;
        }
        groups.push((*first, last))?;
        at = end;
    }
    groups.as_mut_slice().sort_unstable_by_key(|g| g.0);
    let mut files = MeteredVec::with_capacity(meter, groups.len())?;
    for (first, last) in groups.as_slice() {
        files.push(FileSlot {
            name: rows.get(*first).map(|d| d.path.clone()).unwrap_or_default(),
            dir_index: *last,
        })?;
    }
    Ok(files)
}

/// A table as the bundle keeps it, its charge moved into the bundle's.
fn keep<T>(table: MeteredVec<T>, charge: &mut Charge) -> Result<Box<[T]>, Error> {
    let (b, c) = table.into_boxed_slice()?;
    charge.absorb(c);
    Ok(b)
}
