// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! `BundleFile` (`files/BundleFile.py`), read lazily.
//!
//! [`BundleFile::open`] reads the header, blocks-info and directory and
//! decodes no data block, so a registry over thousands of bundles is cheap
//! and [`BundleFile::uncompressed_size`] (the declared total) is known before
//! any decode (for a caller that plans with it, plan section 4.1).
//!
//! **One layout** (plan section 3.6). UnityPy always reads entries from the
//! `b"".join` of every block's *actual* output (`BundleFile.py:173-186`), so
//! an entry's offset depends on the actual length of every block before it.
//! Before a lazy read returns bytes ending at joined offset `X`, every block
//! starting below `X` has a known length (`lengths`): a stored block's is
//! known at open; an unmeasured block wholly before the read is decoded with
//! its output discarded (a measuring decode: charged while it runs, returned
//! after). An entry is the Python slice `data[offset : offset + size]` of the
//! joined data (`File.py:78-81`; [`crate::stream::py_read`]). A lazy read
//! that succeeds returns exactly the slice of [`BundleFile::load_all`]'s
//! joined data, and a block that fails before or inside the read fails it
//! as it fails `load_all`; a block that fails only *after* the read fails
//! `load_all` and not the read (plan section 3.13, difference (c)).
//!
//! **Block outcomes.** A block's measurement is kept once known, failures
//! included: `lengths` holds each block's length or the error its decode
//! gave, so a block that fails is decoded once and every later read that
//! needs it fails at once, as UnityPy fails once. A refusal that depends on
//! what else is held (the meter, the budget) is not kept
//! ([`crate::once::get_or_publish_result`]).
//!
//! **Runs.** The blocks one read overlaps are decoded into one buffer, a run,
//! cached by its block span. A read whose span a cached run covers is a
//! zero-copy slice of it; any other read decodes its own whole span into a
//! new run, so a block that two spans share is decoded once per span.
//! `load_all` decodes every block into one run. [`BundleFile::entry_range`]
//! is a read like any other: it measures, slices a covering cached run when
//! there is one, and otherwise decodes its span into a run it caches, so N
//! ranges in one block decode it once.
//!
//! **Memory and work.** Every decode (first, measuring, repeated) writes
//! into a [`crate::budget::DecodeBuf`] on the input's meter, one segment per
//! block (each capped at `max_block_output`), so whatever the runs hold is
//! charged, and a hostile directory whose spans overlap fails with
//! `LimitKind::InputMemory` instead of growing; every byte a decode writes
//! also counts as its open's work (`LimitKind::DecodeWork`, on the meter,
//! which is this open of the input: a reopen after eviction is a new meter
//! on the same memory, [`crate::limits::Meter::reopen`]), which is not
//! returned while the open lives (a refused growth counts nothing, and
//! writes on several threads never pass the bound together). The header,
//! blocks, directory, `files` and the per-row
//! cells are charged at open; each parsed entry charges its own file, and
//! a kept error charges its heap. Decodes and user codecs run with no
//! lock held: a run is decoded first and then published into `runs`, and an
//! entry is parsed first and then published into its cell
//! ([`crate::once::get_or_publish_result`]). Publishing a run may grow the
//! run table under its mutex, which charges the meter and so calls the
//! budget; the budget's contract forbids it to block or call the core
//! ([`crate::budget::MemoryBudget`]), and a run replaced or dropped under
//! the lock is released after it.
//!
//! UnityPy rules ported exactly (plan section 3.6):
//! - the extra byte after the data flags only for non-UnityFS signatures
//!   (`BundleFile.py:104-106`; rabex reads it for UnityFS, `files/bundlefile/mod.rs:616-618`);
//! - the flag set by engine version (`:113-121`); the decryptor read *before*
//!   the 16-byte alignment (`:123-124`, then `:129-131`; rabex reads it after,
//!   `files/bundlefile/mod.rs:638-645`); the alignment at format 7 and up or a
//!   2019 engine at 2019.4.15 or later;
//! - blocks-info at the end read at `len - compressed` (`:134-137`); padding at
//!   the start only under `ArchiveFlags` with 0x200 (`:170-171`);
//! - short reads: a truncated block is read short and decoded as it is; its
//!   `bytes_read` is what the dump writes as `compressed`;
//! - LZMA's output can outgrow its declared size: the block cap
//!   ([`crate::limits::Limits::max_block_output`]) and the input's meter
//!   follow the actual bytes, never the declared ones.
//!
//! UnityWeb/UnityRaw: format 6 takes the UnityFS layout (P1); every other
//! format is [`crate::Unsupported::WebRawLayout`] until P3. `UnityArchive` is
//! [`crate::Unsupported::UnityArchive`], as UnityPy raises (`:41-42`).
//!
//! Owner: package C.

use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::codec::{Codec, decode_block};
use crate::detect::check_file_type;
use crate::error::{Error, LimitKind, Unsupported};
use crate::files::{File, Parent, ParentKind, parse_file};
use crate::limits::{Charge, Meter, MeteredVec, heap_cost};
use crate::once::{ResultCell, get_or_publish_result};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, Reader, py_read};
use crate::typetree::parse::string_to_null;
use crate::version::{
    FlagsClass, ResolvedVersion, VersionError, flags_class, resolve_bundle, uses_block_alignment,
};

/// Which of UnityPy's two readers a bundle takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutKind {
    /// `read_fs`: UnityFS, and UnityWeb/UnityRaw at format 6.
    Fs,
    /// `read_web_raw`: UnityWeb/UnityRaw at other formats (P3).
    WebRaw,
}

/// The header, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BundleHeader {
    /// `signature`.
    pub signature: RawStr,
    /// UnityPy's `version`: the format number.
    pub format: u32,
    /// `version_player`.
    pub version_player: RawStr,
    /// `version_engine`.
    pub version_engine: RawStr,
    /// The reader taken.
    pub layout: LayoutKind,
    /// `parse_version()`'s result, where loading calls it (the `Fs` layout).
    /// The dump's `engine_version_packed` comes from it whether or not
    /// loading called it (`CANONICAL_DUMP.md` 4.2).
    pub engine_version: Option<Result<ResolvedVersion, VersionError>>,
    /// The data flags (`Fs` layout).
    pub data_flags: Option<u32>,
    /// `ArchiveFlags` or `ArchiveFlagsOld` (`Fs` layout).
    pub flags_class: Option<FlagsClass>,
    /// Whether the 16-byte alignment after the header applied.
    pub block_alignment: bool,
    /// Whether the encryption flag made UnityPy build a decryptor.
    pub encrypted: bool,
    /// The web/raw hash (format 4 and up, not 6).
    pub hash: Option<[u8; 16]>,
    /// The web/raw CRC (format 4 and up, not 6).
    pub crc: Option<u32>,
    /// The blocks-info section's row (`Fs` layout).
    pub blocks_info: Option<BlockInfo>,
}

/// One block (or the blocks-info section).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlockInfo {
    /// Declared decoded size.
    pub uncompressed: u32,
    /// Declared stored size.
    pub compressed: u32,
    /// The bytes actually available and read: below `compressed` when the
    /// file is truncated. The dump writes this as `compressed`.
    pub bytes_read: u32,
    /// Flags: `u16` for a block, the data flags for blocks-info.
    pub flags: u32,
}

/// One directory row.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirEntry {
    /// Offset in the decoded data.
    pub offset: i64,
    /// Size in the decoded data.
    pub size: i64,
    /// Flags (`Fs` layout).
    pub flags: Option<u32>,
    /// The path, as read.
    pub path: RawStr,
}

/// One key of UnityPy's `files` dict: a path at its first position, holding
/// the last directory entry of that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSlot {
    /// The path.
    pub name: RawStr,
    /// The directory index of the entry the dict holds.
    pub dir_index: usize,
}

/// A bundle: header and directory parsed, data decoded on demand.
pub struct BundleFile {
    name: RawStr,
    source: Bytes,
    header: BundleHeader,
    blocks: Box<[BlockInfo]>,
    directory: Box<[DirEntry]>,
    files: Box<[FileSlot]>,
    parent: Option<Parent>,
    /// Each block's outcome, once known: its actual decoded length (a stored
    /// block's at open, any other's when it is first decoded or measured),
    /// or the error its decode gave, kept so it is not decoded again.
    lengths: Box<[ResultCell<u64, Error>]>,
    /// Decoded block runs: `(first block, end block, bytes)`, each run's
    /// charge inside its `Bytes`. Locked only to look up or publish.
    runs: Mutex<MeteredVec<(usize, usize, Bytes)>>,
    /// Parsed entries, by directory index, published once; a kept error
    /// carries the charge for its heap, and a refused charge is not kept.
    entries: Box<[ResultCell<File, Error>]>,
    /// The handle and its tables (header, blocks, directory, `files`, the
    /// parse cells), charged to the input at open, with the engine version's
    /// heap ([`crate::version::UnityVersion::heap_bytes`]; its string is the
    /// header's own slice, never a copy).
    charge: Charge,
    ctx: Arc<FileContext>,
    /// Each block's stored bytes in `source`: the Python slice UnityPy read.
    stored: Box<[Range<usize>]>,
    /// This bundle, for its entries' [`ParentKind::Bundle`].
    this: Weak<BundleFile>,
}

impl std::fmt::Debug for BundleFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleFile")
            .field("name", &self.name)
            .field("header", &self.header)
            .field("blocks", &self.blocks.len())
            .field("directory", &self.directory.len())
            .finish_non_exhaustive()
    }
}

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

    /// The meter of the input it belongs to (of this open of it); its
    /// entries, nested bundles included, charge it too.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The input bytes it was opened on (for telling two handles on one
    /// input apart from two inputs: a resource read through `from`'s own
    /// handle, plan section 3.13).
    #[must_use]
    pub fn source(&self) -> &Bytes {
        &self.source
    }

    /// The name it was opened under.
    #[must_use]
    pub fn name(&self) -> &RawStr {
        &self.name
    }

    /// The header.
    #[must_use]
    pub fn header(&self) -> &BundleHeader {
        &self.header
    }

    /// The blocks, in file order (`Fs` layout).
    #[must_use]
    pub fn blocks(&self) -> &[BlockInfo] {
        &self.blocks
    }

    /// The directory, in file order, repeated paths kept.
    #[must_use]
    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }

    /// UnityPy's `files` dict, in order.
    #[must_use]
    pub fn files(&self) -> &[FileSlot] {
        &self.files
    }

    /// The container this bundle is an entry of.
    #[must_use]
    pub fn parent(&self) -> Option<&Parent> {
        self.parent.as_ref()
    }

    /// The context it was opened with.
    #[must_use]
    pub fn context(&self) -> &Arc<FileContext> {
        &self.ctx
    }

    /// The sum of the blocks' *declared* decoded sizes: what a full decode is
    /// expected to cost. The actual total can differ (module docs).
    #[must_use]
    pub fn uncompressed_size(&self) -> u64 {
        self.blocks.iter().map(|b| u64::from(b.uncompressed)).sum()
    }

    /// Directory entry `dir_index`'s bytes: the Python slice
    /// `data[offset : offset + size]` of the joined data (size -1 to the end,
    /// negative indexes from the end, short at the end). Measures every block
    /// before the slice first (module docs).
    pub fn entry_bytes(&self, dir_index: usize) -> Result<Bytes, Error> {
        let span = self.entry_span(dir_index)?;
        self.read_span(span.start, span.end)
    }

    /// A byte range of an entry (a `.resS` read), short at the end of the
    /// entry as a Python slice. It measures every block before the range as
    /// any read does, slices a cached run that covers the range's blocks, and
    /// otherwise decodes those blocks into a run it caches, so later ranges
    /// in the same blocks are slices of it.
    pub fn entry_range(&self, dir_index: usize, range: Range<u64>) -> Result<Bytes, Error> {
        let span = self.entry_span(dir_index)?;
        let lo = span.start.saturating_add(range.start).min(span.end);
        let hi = span.start.saturating_add(range.end).clamp(lo, span.end);
        self.read_span(lo, hi)
    }

    /// Directory entry `dir_index`, detected and parsed (`parse_file`) and
    /// published once ([`crate::once::get_or_publish_result`]); under
    /// contention two callers may both parse it, and the loser's result (and
    /// its charge) is dropped. A parse the meter or the budget refuses is
    /// returned and not kept.
    pub fn entry(&self, dir_index: usize) -> Result<File, Error> {
        let cell = self
            .entries
            .get(dir_index)
            .ok_or_else(|| no_entry(dir_index))?;
        get_or_publish_result(cell, self.meter(), || self.parse_entry(dir_index)).cloned()
    }

    /// UnityPy's eager load (`File.read_files`, `File.py:76-90`): every block
    /// decoded, every directory entry parsed in order (a path repeated in the
    /// directory included: its earlier entry can fail the bundle though the
    /// `files` dict keeps the later one); the first failure fails the bundle.
    pub fn load_all(&self) -> Result<(), Error> {
        // UnityPy files/BundleFile.py:168-178: `b"".join` of every block.
        self.read_span(0, u64::MAX)?;
        // UnityPy files/File.py:76-90
        for i in 0..self.directory.len() {
            if let File::Bundle(b) = self.entry(i)? {
                b.load_all().map_err(|e| self.entry_error(i, e))?;
            }
        }
        Ok(())
    }

    /// The entry's slice of the joined data, as joined offsets: `offset` and
    /// `size` as UnityPy's `reader.Position = offset; reader.read(size)`. A
    /// non-negative offset with a positive size needs no total (the end is
    /// clamped as the read goes); anything else measures every block.
    fn entry_span(&self, dir_index: usize) -> Result<Range<u64>, Error> {
        let e = self
            .directory
            .get(dir_index)
            .ok_or_else(|| no_entry(dir_index))?;
        let u = |v: i64| u64::try_from(v).unwrap_or(0);
        if e.size == 0 {
            return Ok(0..0);
        }
        if e.offset >= 0 && e.size > 0 {
            return Ok(u(e.offset)..u(e.offset).saturating_add(u(e.size)));
        }
        let total = self.measure_all()?;
        let len = usize::try_from(total).unwrap_or(usize::MAX);
        let (range, _) = py_read(len, i128::from(e.offset), e.size);
        Ok(u64::try_from(range.start).unwrap_or(u64::MAX)
            ..u64::try_from(range.end).unwrap_or(u64::MAX))
    }

    fn entry_error(&self, dir_index: usize, e: Error) -> Error {
        match e {
            Error::Entry { .. } | Error::Limit(_) | Error::Budget(_) => e,
            e => Error::Entry {
                name: self
                    .directory
                    .get(dir_index)
                    .map(|d| d.path.clone())
                    .unwrap_or_default(),
                source: Box::new(e),
            },
        }
    }

    /// `parse_file` on the entry's bytes, its detection included, as
    /// `File.read_files` calls it.
    fn parse_entry(&self, dir_index: usize) -> Result<File, Error> {
        let e = self
            .directory
            .get(dir_index)
            .ok_or_else(|| no_entry(dir_index))?;
        let bytes = self.entry_bytes(dir_index)?;
        let wrap = |e| self.entry_error(dir_index, e);
        let ty = check_file_type(&bytes, &self.ctx.limits).map_err(|d| wrap(Error::Detect(d)))?;
        let parent = Parent {
            name: self.name.clone(),
            kind: ParentKind::Bundle {
                version_engine: self.header.version_engine.clone(),
                bundle: self.this.clone(),
            },
            depth: self
                .parent
                .as_ref()
                .map_or(0, |p| p.depth)
                .saturating_add(1),
        };
        parse_file(
            e.path.clone(),
            bytes,
            ty,
            Some(parent),
            self.meter(),
            &self.ctx,
        )
        .map_err(wrap)
    }

    /// Block `i`'s length, if known (an error is the block's kept failure).
    fn known(&self, i: usize) -> Option<Result<u64, Error>> {
        Some(
            self.lengths
                .get(i)?
                .get()?
                .as_ref()
                .map(|n| *n)
                .map_err(|k| k.error().clone()),
        )
    }

    /// Decodes block `i` onto the end of `out` and records its length (a
    /// failure the input decides is kept, a refusal is not): the length.
    fn decode_into(&self, i: usize, out: &mut DecodeBuf) -> Result<u64, Error> {
        let before = out.len();
        let (Some(b), Some(range)) = (self.blocks.get(i), self.stored.get(i)) else {
            return Err(no_entry(i));
        };
        let input = self.source.slice(range.clone());
        let got = decode_block_into(
            &self.ctx,
            self.meter(),
            input,
            b.uncompressed,
            u32::from(b.flags_u16()),
            i,
            out,
        )
        .map(|()| u64::try_from(out.len() - before).unwrap_or(u64::MAX));
        if got.is_err() {
            out.truncate(before);
        }
        let cell = self.lengths.get(i).ok_or_else(|| no_entry(i))?;
        let mine = got?;
        let published = *get_or_publish_result(cell, self.meter(), || Ok(mine))?;
        if published != mine {
            return Err(Error::Decompress {
                codec: Codec::from_flags(u32::from(b.flags_u16())),
                detail: format!("block {i} decoded to {mine} bytes, before to {published}").into(),
            });
        }
        Ok(mine)
    }

    /// Decodes block `i` and records the outcome, keeping a failure the
    /// input decides.
    fn decode_or_keep(&self, i: usize, out: &mut DecodeBuf) -> Result<u64, Error> {
        let got = self.decode_into(i, out);
        if let (Err(e), Some(cell)) = (&got, self.lengths.get(i)) {
            // A refusal (meter, budget) is not kept; anything else is.
            let _ = get_or_publish_result(cell, self.meter(), || Err(e.clone()));
        }
        got
    }

    /// A run's buffer with room for `initial` bytes, charged before it is
    /// allocated. The room is memory; each block's decode counts what it
    /// writes into it as the open's work (`DecodeBuf`, round 6).
    fn new_buf(&self, initial: u64) -> Result<DecodeBuf, Error> {
        Ok(DecodeBuf::new(
            self.meter(),
            initial,
            u64::MAX,
            LimitKind::BlockOutput,
        )?)
    }

    /// Every block's length, measured: unknown blocks are decoded with their
    /// output discarded. The joined total.
    fn measure_all(&self) -> Result<u64, Error> {
        let mut total = 0u64;
        for i in 0..self.blocks.len() {
            let n = if let Some(n) = self.known(i) {
                n?
            } else {
                let mut scratch = self.new_buf(0)?;
                self.decode_or_keep(i, &mut scratch)?
            };
            total = total.saturating_add(n);
        }
        Ok(total)
    }

    /// The joined bytes `[lo, min(hi, total))`, as one pass over the blocks:
    /// every block starting below `hi` gets a known length (decoding the
    /// unknown ones; one wholly before `lo` is decoded with its output
    /// discarded), the blocks overlapping the read form its span, and the
    /// span is a cached run's slice, a slice of the input (consecutive
    /// stored blocks), or decoded into a new run that is published.
    fn read_span(&self, lo: u64, hi: u64) -> Result<Bytes, Error> {
        let n = self.blocks.len();
        let mut acc = 0u64;
        let mut first: Option<(usize, u64)> = None;
        // The run being decoded, from block `first`.
        let mut buf: Option<DecodeBuf> = None;
        let mut i = 0;
        while i < n && acc < hi {
            let len = if let Some(len) = self.known(i) {
                let len = len?;
                if acc.saturating_add(len) > lo && first.is_none() {
                    first = Some((i, acc));
                }
                if let Some(b) = buf.as_mut() {
                    self.decode_or_keep(i, b)?;
                }
                len
            } else if first.is_none() {
                // Perhaps wholly before the read: decoded on its own, kept
                // as the run's start only when it reaches the read.
                let mut scratch = self.new_buf(0)?;
                let len = self.decode_or_keep(i, &mut scratch)?;
                if acc.saturating_add(len) > lo {
                    first = Some((i, acc));
                    buf = Some(scratch);
                }
                len
            } else {
                if buf.is_none() {
                    // Start the run with the span's known blocks so far.
                    let mut b = self.new_buf(0)?;
                    if let Some((f, _)) = first {
                        for k in f..i {
                            self.decode_or_keep(k, &mut b)?;
                        }
                    }
                    buf = Some(b);
                }
                let b = buf.as_mut().ok_or_else(|| no_entry(i))?;
                self.decode_or_keep(i, b)?
            };
            acc = acc.saturating_add(len);
            i += 1;
        }
        let Some((first, first_at)) = first else {
            return Ok(Bytes::new());
        };
        let end = i;
        let from = usize::try_from(lo.saturating_sub(first_at)).unwrap_or(usize::MAX);
        let to = usize::try_from(hi.min(acc) - first_at).unwrap_or(usize::MAX);
        let run = if let Some(b) = buf {
            self.publish(first, end, b.into_bytes()?)
        } else if let Some(bytes) = self.cached(first, end, first_at)? {
            bytes
        } else if let Some(bytes) = self.stored_span(first, end) {
            bytes
        } else {
            let mut b = self.new_buf(acc - first_at)?;
            for k in first..end {
                self.decode_or_keep(k, &mut b)?;
            }
            self.publish(first, end, b.into_bytes()?)
        };
        let to = to.min(run.len());
        Ok(run.slice(from.min(to)..to))
    }

    /// The joined offset where block `first` starts (every length before it
    /// is known when a run starting there exists).
    fn joined_start(&self, first: usize) -> Result<u64, Error> {
        let mut at = 0u64;
        for k in 0..first {
            at = at.saturating_add(self.known(k).unwrap_or(Ok(0))?);
        }
        Ok(at)
    }

    /// A cached run covering blocks `first..end`, sliced to start at block
    /// `first` (joined offset `first_at`).
    fn cached(&self, first: usize, end: usize, first_at: u64) -> Result<Option<Bytes>, Error> {
        let hit = {
            let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            runs.as_slice()
                .iter()
                .find(|(f, e, _)| *f <= first && end <= *e)
                .map(|(f, _, b)| (*f, b.clone()))
        };
        let Some((f, bytes)) = hit else {
            return Ok(None);
        };
        let skip = first_at - self.joined_start(f)?;
        let skip = usize::try_from(skip).unwrap_or(usize::MAX).min(bytes.len());
        Ok(Some(bytes.slice(skip..)))
    }

    /// Blocks `first..end` as one slice of the input, when every one is
    /// stored and each follows the previous in the input.
    fn stored_span(&self, first: usize, end: usize) -> Option<Bytes> {
        let mut at: Option<Range<usize>> = None;
        for k in first..end {
            let b = self.blocks.get(k)?;
            if Codec::from_flags(u32::from(b.flags_u16())) != Codec::None {
                return None;
            }
            let r = self.stored.get(k)?;
            at = match at {
                None => Some(r.clone()),
                Some(a) if a.end == r.start || r.is_empty() => Some(a.start..a.end.max(r.end)),
                Some(_) => return None,
            };
        }
        at.map(|r| self.source.slice(r))
    }

    /// Publishes the run of blocks `first..end`: an equal run already there
    /// answers instead, and runs it covers are dropped (after the lock).
    fn publish(&self, first: usize, end: usize, bytes: Bytes) -> Bytes {
        let mut dropped: Vec<(usize, usize, Bytes)> = Vec::new();
        let out = {
            let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, _, b)) = runs
                .as_slice()
                .iter()
                .find(|(f, e, _)| *f == first && *e == end)
            {
                b.clone()
            } else {
                let all = runs.as_mut_slice();
                let mut k = 0;
                let mut keep_n = all.len();
                while k < keep_n {
                    if all.get(k).is_some_and(|(f, e, _)| first <= *f && *e <= end) {
                        all.swap(k, keep_n - 1);
                        keep_n -= 1;
                    } else {
                        k += 1;
                    }
                }
                let tail: Vec<(usize, usize, Bytes)> = all
                    .get_mut(keep_n..)
                    .map(|t| {
                        t.iter_mut()
                            .map(|(f, e, b)| (*f, *e, std::mem::take(b)))
                            .collect()
                    })
                    .unwrap_or_default();
                dropped = tail;
                runs.truncate(keep_n);
                // A refused growth of the table keeps the run uncached.
                let _ = runs.push((first, end, bytes.clone()));
                bytes
            }
        };
        drop(dropped);
        out
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
    perm.as_mut_slice().sort_by(|a, b| path(a).cmp(path(b)));
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

/// `decompress_data` (`BundleFile.py:509-521`) on one block or the
/// blocks-info section, as a whole buffer: a slice of the input for a stored
/// one, else a charged decode.
fn decode_one(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
) -> Result<Bytes, Error> {
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let (which, codec) = codec_for(ctx, flags)?;
    let input = decrypted(ctx, meter, input, flags, which, index)?;
    match decode_block(
        which,
        codec.as_ref(),
        &input,
        uncompressed,
        &mut out,
        ctx.limits.max_block_output,
    )
    .map_err(|e| with_codec(e, which))?
    {
        Some(slice) => Ok(slice),
        None => Ok(out.into_bytes()?),
    }
}

/// `decompress_data` on one block, appended to `out`.
fn decode_block_into(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
    out: &mut DecodeBuf,
) -> Result<(), Error> {
    let (which, codec) = codec_for(ctx, flags)?;
    let input = decrypted(ctx, meter, input, flags, which, index)?;
    if let Some(slice) = decode_block(
        which,
        codec.as_ref(),
        &input,
        uncompressed,
        out,
        ctx.limits.max_block_output,
    )
    .map_err(|e| with_codec(e, which))?
    {
        out.extend_from_slice(&slice)?;
    }
    Ok(())
}

/// The registered codec for `flags & 0x3F`: LZHAM and an unknown type
/// raise (`CompressionHelper.py:234-235`, `BundleFile.py:515-521`).
fn codec_for(
    ctx: &FileContext,
    flags: u32,
) -> Result<(Codec, Arc<dyn crate::codec::BlockCodec>), Error> {
    let which = Codec::from_flags(flags);
    match ctx.codecs.get(which) {
        Some(c) => Ok((which, c.clone())),
        None if which == Codec::Lzham => Err(Error::Unsupported(Unsupported::Lzham)),
        None => Err(Error::Unsupported(Unsupported::Codec(which.id()))),
    }
}

/// The block decrypted as UnityPy decrypts it (`flags & 0x100`, not
/// stored), in place in a charged copy; the input as it is otherwise.
// UnityPy files/BundleFile.py:512-513
fn decrypted(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    flags: u32,
    which: Codec,
    index: usize,
) -> Result<Bytes, Error> {
    let Some(d) = ctx.decryptor.as_ref() else {
        return Ok(input);
    };
    if flags & 0x100 == 0 || which == Codec::None {
        return Ok(input);
    }
    let mut copy = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let block = copy.grow_zeroed(input.len())?;
    block.copy_from_slice(&input);
    d.decrypt(block, index)?;
    Ok(copy.into_bytes()?)
}

/// A decode error named by the block's codec (one codec may serve two ids).
fn with_codec(e: Error, which: Codec) -> Error {
    match e {
        Error::Decompress { detail, .. } => Error::Decompress {
            codec: which,
            detail,
        },
        e => e,
    }
}

fn no_entry(i: usize) -> Error {
    Error::Malformed {
        what: "bundle index",
        detail: format!("no row {i}").into(),
    }
}

/// A table as the bundle keeps it, its charge moved into the bundle's.
fn keep<T>(table: MeteredVec<T>, charge: &mut Charge) -> Result<Box<[T]>, Error> {
    let (b, c) = table.into_boxed_slice()?;
    charge.absorb(c);
    Ok(b)
}

impl BlockInfo {
    /// A block's `u16` flags.
    fn flags_u16(&self) -> u16 {
        u16::try_from(self.flags & 0xFFFF).unwrap_or(0)
    }
}
