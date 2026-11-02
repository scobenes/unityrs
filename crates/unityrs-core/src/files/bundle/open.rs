// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! [`BundleFile::open`]: the header, blocks-info and directory, read as
//! `read_fs` reads them, UnityPy's `files` dict, and where each block's
//! stored bytes are. No data block is decoded.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use super::decode::{BlockContext, decode_one};
use super::index::{Ends, RunIndex};
use super::{BlockInfo, BundleFile, BundleHeader, DirEntry, FileSlot, LayoutKind};
use crate::codec::Codec;
use crate::detect::raw_by_name;
use crate::error::{Error, Unsupported};
use crate::files::{File, Parent};
use crate::limits::{Charge, Meter, MeteredVec, heap_cost};
use crate::once::ResultCell;
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, Reader, ReaderKind};
use crate::typetree::parse::string_to_null;
use crate::version::{
    FlagsClass, ResolvedVersion, flags_class, resolve_bundle, uses_block_alignment,
};

impl BundleFile {
    /// Reads the header, blocks-info and directory of bytes (the memory
    /// reader's rules). Decodes no data block. Everything it keeps is
    /// charged to `meter`, the input's.
    pub fn open(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        Self::open_with_reader(name, data, parent, meter, ctx, ReaderKind::Memory)
    }

    /// [`Self::open`] by the rules of the reader UnityPy would read `data`
    /// with ([`ReaderKind::Stream`] for a top-level file loaded from a path):
    /// the header, the blocks-info section and the blocks' stored bytes are
    /// read through it; the blocks-info table and the entries are memory
    /// (UnityPy decodes them into bytes, `files/BundleFile.py:141-142`, `:173-186`).
    pub fn open_with_reader(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
        reader: ReaderKind,
    ) -> Result<Arc<Self>, Error> {
        let mut charge = meter.charge(heap_cost(
            u64::try_from(std::mem::size_of::<Self>() + 16).unwrap_or(u64::MAX),
        ))?;
        let mut r = Reader::with_kind(&data, Endian::Big, reader);
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
        } = read_fs_header(&mut r, &data, &mut charge, ctx)?;
        let blocks_ctx = BlockContext::new(ctx, encrypted);
        // :133-139: at the end, `Position = Length - compressedSize` (which a
        // stream refuses when negative), the read, and back to the start.
        let start = r.position();
        let info_range = if data_flags & 0x80 != 0 {
            let len = i128::try_from(data.len()).unwrap_or(i128::MAX);
            r.set_position(len - i128::from(info_compressed))?;
            let range = r.read_range(i64::from(info_compressed))?;
            r.set_position(start)?;
            range
        } else {
            r.read_range(i64::from(info_compressed))?
        };
        let info_read = u32::try_from(info_range.len()).unwrap_or(u32::MAX);
        let blocks_info = BlockInfo {
            uncompressed: info_uncompressed,
            compressed: info_compressed,
            bytes_read: info_read,
            flags: data_flags,
        };
        let info = decode_one(
            &blocks_ctx,
            meter,
            data.slice(info_range),
            info_uncompressed,
            data_flags,
            0,
        )?;
        let (mut blocks, directory) = read_blocks_info(&info, meter)?;
        // :170-171
        if class == FlagsClass::New && data_flags & 0x200 != 0 {
            r.align(16)?;
        }
        // :173-184: the blocks, read in order from here.
        let (stored, lengths) = stored_blocks(&mut blocks, &mut r, meter, ctx)?;
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
        let twins = keep(twins(&directory, meter)?, &mut charge)?;
        let stored = keep(stored, &mut charge)?;
        let (ends, c) = Ends::new(blocks.len(), meter)?;
        charge.absorb(c);
        Ok(Arc::new_cyclic(|this| Self {
            name,
            source: data,
            header,
            blocks,
            directory,
            files,
            parent,
            lengths,
            ends,
            runs: Mutex::new(RunIndex::new(meter)),
            entries,
            twins,
            charge,
            ctx: blocks_ctx,
            stored,
            this: this.clone(),
        }))
    }
}

/// Where each block's stored bytes are (`reader.read_bytes(compressedSize)`
/// in order from `r`, by its reader's rules: a size is never negative, so
/// the two readers read the same bytes), its `bytes_read` set; and the length cells, a stored
/// block's filled with what was read (`lambda cd, _ucs: cd`) when stored
/// blocks are their own bytes ([`crate::codec::Codecs::stores_as_is`]) and
/// what was read is within the block cap
/// ([`crate::limits::Limits::max_block_output`]). A stored block past the
/// cap is left to its codec, which refuses it with
/// [`crate::LimitKind::BlockOutput`] as it does in `load_all`, so a lazy read
/// fails it as the eager load does.
#[allow(clippy::type_complexity)]
fn stored_blocks(
    blocks: &mut MeteredVec<BlockInfo>,
    r: &mut Reader<'_>,
    meter: &Arc<Meter>,
    ctx: &FileContext,
) -> Result<(MeteredVec<Range<usize>>, MeteredVec<ResultCell<u64, Error>>), Error> {
    let (as_is, cap) = (ctx.codecs.stores_as_is(), ctx.limits.max_block_output);
    let mut stored = MeteredVec::new(meter);
    let mut lengths = MeteredVec::new(meter);
    for b in blocks.as_mut_slice() {
        let range = r.read_range(i64::from(b.compressed))?;
        b.bytes_read = u32::try_from(range.len()).unwrap_or(u32::MAX);
        stored.push(range)?;
        let cell = ResultCell::new();
        let read = u64::from(b.bytes_read);
        if as_is && Codec::from_flags(u32::from(b.flags_u16())) == Codec::None && read <= cap {
            let _ = cell.set(Ok(read));
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
/// the blocks-info section (`files/BundleFile.py:36-51`, `:96-131`); leaves `r` where
/// blocks-info would start. Its strings' searches and its engine version's
/// parse are the work of `charge`'s meter, the input's, and the version's
/// heap is charged to `charge`, the bundle's handle.
fn read_fs_header(
    r: &mut Reader<'_>,
    data: &Bytes,
    charge: &mut Charge,
    ctx: &FileContext,
) -> Result<FsHeader, Error> {
    let meter = charge.meter().clone();
    let meter = &*meter;
    // UnityPy files/BundleFile.py:36-39
    let signature = string_to_null(r, data, meter)?;
    let format = r.read_u32()?;
    let version_player = string_to_null(r, data, meter)?;
    let version_engine = string_to_null(r, data, meter)?;
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
    // :97-106
    let _size = r.read_i64()?;
    let info_compressed = r.read_u32()?;
    let info_uncompressed = r.read_u32()?;
    let data_flags = r.read_u32()?;
    if signature.as_bytes() != b"UnityFS" {
        r.read_i8()?;
    }
    // :108-121
    let engine = resolve_bundle(&version_engine, &ctx.version, charge)?;
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
            r.set_position(r.position() + 1)?;
        }
    }
    // :129-131
    let block_alignment = uses_block_alignment(format, &engine.version)?;
    if block_alignment {
        r.align(16)?;
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

/// The blocks-info and directory tables (`files/BundleFile.py:141-165`), read
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
        let path = string_to_null(&mut r, info, meter)?;
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

/// Each row's first twin (the `bundle` module docs, "Twins"): the first row,
/// in directory order, whose offset, size and name rule
/// ([`crate::detect::raw_by_name`]) are the row's own, or the row itself.
/// Computed by sorting the rows' keys, the scratch charged.
fn twins(rows: &[DirEntry], meter: &Arc<Meter>) -> Result<MeteredVec<u32>, Error> {
    let mut keys: MeteredVec<(i64, i64, bool, u32)> = MeteredVec::with_capacity(meter, rows.len())?;
    let mut twins: MeteredVec<u32> = MeteredVec::with_capacity(meter, rows.len())?;
    for (i, d) in rows.iter().enumerate() {
        // A directory holds at most `i32::MAX` rows (its count is an `i32`).
        let i = u32::try_from(i).unwrap_or(u32::MAX);
        keys.push((d.offset, d.size, raw_by_name(d.path.as_bytes()), i))?;
        twins.push(i)?;
    }
    keys.as_mut_slice().sort_unstable();
    let mut first = None;
    for &(offset, size, raw, i) in keys.as_slice() {
        match first {
            Some((key, f)) if key == (offset, size, raw) => {
                if let Some(t) = usize::try_from(i)
                    .ok()
                    .and_then(|i| twins.as_mut_slice().get_mut(i))
                {
                    *t = f;
                }
            }
            _ => first = Some(((offset, size, raw), i)),
        }
    }
    Ok(twins)
}

/// A table as the bundle keeps it, its charge moved into the bundle's.
fn keep<T>(table: MeteredVec<T>, charge: &mut Charge) -> Result<Box<[T]>, Error> {
    let (b, c) = table.into_boxed_slice()?;
    charge.absorb(c);
    Ok(b)
}
