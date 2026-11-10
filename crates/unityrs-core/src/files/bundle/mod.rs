// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! `BundleFile` (`files/BundleFile.py`), read lazily.
//!
//! [`BundleFile::open`] reads the header, blocks-info and directory and
//! decodes no data block, so a registry over thousands of bundles is cheap
//! and [`BundleFile::uncompressed_size`] (the declared total) is known before
//! any decode (for a caller that plans with it, plan section 4.1).
//!
//! **One layout** (plan section 3.6). UnityPy always reads entries from the
//! `b"".join` of every block's *actual* output (`files/BundleFile.py:173-186`), so
//! an entry's offset depends on the actual length of every block before it.
//! Before a lazy read returns bytes ending at joined offset `X`, every block
//! starting below `X` has a known length (`lengths`): a stored block's is
//! known at open when slot 0 holds the built-in stored codec
//! ([`crate::codec::Codecs::stores_as_is`]) and its bytes are within the
//! block cap ([`crate::limits::Limits::max_block_output`], which a stored
//! block meets on every path, as it does in `load_all`); an unmeasured
//! block wholly before the read is decoded with
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
//! cached by its block span (a span of stored blocks that are their own
//! bytes is one slice of the input, cached as a run too). A read whose span
//! a cached run covers is a zero-copy slice of it; any other read decodes
//! its own whole span into a new run, so a block that two spans share is
//! decoded once per span. `load_all` decodes every block into one run.
//! [`BundleFile::entry_range`] is a read like any other: it measures, slices
//! a covering cached run when there is one, and otherwise decodes its span
//! into a run it caches, so N ranges in one block decode it once.
//!
//! **What a read costs** (`index`). The bundle keeps where each block of the
//! known prefix ends (the prefix sums of the known lengths, in a table
//! charged at open), and its cached runs in an index by first block (a
//! B-tree charged a share per run), in which no run contains another. A read
//! finds its first block and its span's end by binary search of the prefix
//! and a covering run by one search of the index, extends the prefix only
//! past its end (so each block is learned once per open), and decodes only
//! the blocks of a span no run covers: O(log B + log R) steps besides the
//! blocks it learns or decodes, for B blocks and R runs. Every step counts
//! as the open's work, 8 bytes a step (a row a search reads, a block a read
//! learns, decodes again or checks for a stored slice, a run a publish
//! drops), so K reads over B blocks count O(K log B) steps, where each read
//! walked the blocks from block 0 and scanned every run, uncounted
//! (`tests/bundles/runs.rs`).
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
//! a kept error charges its heap. An entry's cell keeps its parse's
//! failure, so the row is not parsed again, but every directory row is its
//! own cell, and rows over one span repeat one parse: what the meter granted
//! a parse that ends in an error the cell keeps, on its own thread, counts
//! as the open's work too, so K such rows stop at the open's `DecodeWork`
//! limit instead of costing K parses no limit sees (round 4;
//! `tests/bundles/work.rs`). A parse that succeeds keeps its tables (each
//! row pays for them), and what its AssetBundle read builds and drops is
//! counted by the read (`files::serialized`, "Work"; round 5). A failure the
//! cell does not keep (the meter's or the budget's refusal) is
//! [`crate::once`]'s to count. A block's stored bytes count too, before a
//! codec reads them, as credit for what it writes (`decode`, "The input is
//! work"; round 6: LZ4 scans a whole block before it writes, so a nested
//! bundle whose blocks-info or block fails at its end cost each row over it
//! a scan no count saw).
//!
//! **Twins.** Rows with the same offset and size read the same bytes, and
//! when their names agree on the one rule that changes what a parse builds
//! from given bytes (a raw suffix, [`crate::detect::parsed_as`]) they parse
//! alike, but for the name each file and each wrapped error carries. Such
//! rows are twins, and they answer a failure as one cell does: the first
//! failure the input decides kept for any of them (the first twin's cell
//! holds it) is every twin's answer, under each one's own name, so K rows
//! over one failing span parse it once (round 6). A twin that parses
//! successfully parses for itself, as its file carries its name. The table
//! of first twins is a sort of the directory's keys at open, charged with
//! the other per-row tables.
//!
//! Decodes and user codecs run with no
//! lock held: a run is decoded first and then published into `runs`, and an
//! entry is parsed first and then published into its cell
//! ([`crate::once::get_or_publish_result`]). Publishing a run may grow the
//! run index under its mutex, which charges the meter and so calls the
//! budget; the budget's contract forbids it to block or call the core
//! ([`crate::budget::MemoryBudget`]), and a run replaced or dropped under
//! the lock is released after it. The known prefix takes no lock: a reader
//! loads how many blocks it holds and searches them while another thread
//! extends it.
//!
//! UnityPy rules ported exactly (plan section 3.6):
//! - the extra byte after the data flags only for non-UnityFS signatures
//!   (`files/BundleFile.py:104-106`; rabex reads it for UnityFS, `files/bundlefile/mod.rs:616-618`);
//! - the flag set by engine version (`files/BundleFile.py:113-121`); the decryptor read *before*
//!   the 16-byte alignment (`:123-124`, then `:129-131`; rabex reads it after,
//!   `files/bundlefile/mod.rs:638-645`); the alignment at format 7 and up or a
//!   2019 engine at 2019.4.15 or later;
//! - blocks-info at the end read at `len - compressed` (`files/BundleFile.py:134-137`); padding at
//!   the start only under `ArchiveFlags` with 0x200 (`:170-171`);
//! - short reads: a truncated block is read short and decoded as it is; its
//!   `bytes_read` is what the dump writes as `compressed`;
//! - LZMA's output can outgrow its declared size: the block cap
//!   ([`crate::limits::Limits::max_block_output`]) and the input's meter
//!   follow the actual bytes, never the declared ones.
//!
//! UnityWeb/UnityRaw: format 6 takes the UnityFS layout (P1); every other
//! format is [`crate::Unsupported::WebRawLayout`] until P3. `UnityArchive` is
//! [`crate::Unsupported::UnityArchive`], as UnityPy raises (`files/BundleFile.py:41-42`).
//!
//! Owner: package C.

use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use bytes::Bytes;

use crate::detect::{DetectError, check_entry_type};
use crate::error::Error;
use crate::files::{File, Parent, ParentKind, parse_file};
use crate::limits::{Charge, GrantScope, Meter};
use crate::once::{CellError, ResultCell, get_or_publish_result};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::py_read;
use crate::version::{FlagsClass, ResolvedVersion, VersionError};
use index::{Ends, RunIndex};

mod ahead;
mod counts;
mod decode;
mod index;
mod open;
mod runs;

pub use counts::BundleCounts;
use counts::DecodeCounts;
use decode::BlockContext;

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
    /// block's at open while stored blocks are their own bytes,
    /// `Codecs::stores_as_is`; any other's when it is first decoded or
    /// measured), or the error its decode gave, kept so it is not decoded
    /// again.
    lengths: Box<[ResultCell<u64, Error>]>,
    /// Where each block of the known prefix ends in the joined data, found
    /// by binary search; charged at open, extended with no lock.
    ends: Ends,
    /// The cached runs by first block, each run's charge inside its `Bytes`
    /// and the index's own in it. Locked only to look up or publish.
    runs: Mutex<RunIndex>,
    /// What the block decodes wrote, dropped and wrote again
    /// ([`Self::counts`]).
    counts: DecodeCounts,
    /// Parsed entries, by directory index, published once; a kept error
    /// carries the charge for its heap, and a refused charge is not kept.
    entries: Box<[ResultCell<File, Error>]>,
    /// Each row's first twin (module docs, "Twins"): the first row, in
    /// directory order, with its offset, size and name rule; a row with no
    /// earlier twin names itself.
    twins: Box<[u32]>,
    /// The handle and its tables (header, blocks, directory, `files`, the
    /// parse cells), charged to the input at open, with the engine version's
    /// heap ([`crate::version::UnityVersion::heap_bytes`], charged by the chain
    /// before it was allocated; its string is the
    /// header's own slice, never a copy).
    charge: Charge,
    /// The caller's context, and whether the header built a decryptor
    /// (UnityPy's `self.decryptor`): what the blocks decode with.
    ctx: BlockContext,
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
        self.ctx.file()
    }

    /// The sum of the blocks' *declared* decoded sizes: what a full decode is
    /// expected to cost. The actual total can differ (module docs).
    #[must_use]
    pub fn uncompressed_size(&self) -> u64 {
        self.blocks.iter().map(|b| u64::from(b.uncompressed)).sum()
    }

    /// What this bundle's block reads have decoded, dropped, decoded again
    /// and cached so far ([`BundleCounts`]; a nested bundle counts its own):
    /// what plan section 8's R2 and R3 measure. Takes the run index's lock
    /// for the runs' share.
    #[must_use]
    pub fn counts(&self) -> BundleCounts {
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        let n = u64::try_from(runs.len()).unwrap_or(u64::MAX);
        self.counts.snapshot(n, runs.bytes())
    }

    /// Directory entry `dir_index`'s bytes: the Python slice
    /// `data[offset : offset + size]` of the joined data (size -1 to the end,
    /// negative indexes from the end, short at the end). Measures every block
    /// before the slice first (module docs).
    pub fn entry_bytes(&self, dir_index: usize) -> Result<Bytes, Error> {
        let span = self.entry_span(dir_index)?;
        self.read_span(span.start, span.end)
    }

    /// The length of the buffer this bundle's own bytes are a slice of,
    /// which a slice of a stored block keeps alive (`Parent::pinned`; its
    /// own length for a top-level bundle).
    #[must_use]
    pub fn pinned(&self) -> u64 {
        let own = u64::try_from(self.source.len()).unwrap_or(u64::MAX);
        self.parent.as_ref().map_or(own, |p| p.pinned.max(own))
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
        // UnityPy files/BundleFile.py:173-184: `b"".join` of every block.
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
                source: Arc::new(e),
            },
        }
    }

    /// The entry's parse (module docs, "Twins"): its first twin's kept
    /// failure, when there is one; else its own parse, whose failure the
    /// input decides is kept for its first twin too.
    fn parse_entry(&self, dir_index: usize) -> Result<File, Error> {
        let twin = self
            .twins
            .get(dir_index)
            .and_then(|t| usize::try_from(*t).ok())
            .filter(|t| *t != dir_index);
        let twin_cell = twin.and_then(|t| Some((t, self.entries.get(t)?)));
        if let Some((t, cell)) = twin_cell
            && let Some(Err(kept)) = cell.get()
        {
            return Err(self.as_row(kept.error(), t, dir_index));
        }
        let got = self.parse_own(dir_index);
        if let (Some((t, cell)), Err(e)) = (twin_cell, &got)
            && !e.is_transient()
        {
            let _ = get_or_publish_result(cell, self.meter(), || Err(self.as_row(e, dir_index, t)));
        }
        got
    }

    /// `e`, the failure of row `from`, as row `to`'s: the same failure, under
    /// `to`'s name where [`Self::entry_error`] gave it `from`'s. Only that
    /// wrapping builds an [`Error::Entry`]: no parse returns one.
    fn as_row(&self, e: &Error, from: usize, to: usize) -> Error {
        let path = |i: usize| self.directory.get(i).map(|d| &d.path);
        match e {
            Error::Entry { name, source } if path(from) == Some(name) => Error::Entry {
                name: path(to).cloned().unwrap_or_default(),
                source: source.clone(),
            },
            e => e.clone(),
        }
    }

    /// `parse_file` on the entry's bytes, its detection included, as
    /// `File.read_files` calls it.
    fn parse_own(&self, dir_index: usize) -> Result<File, Error> {
        let e = self
            .directory
            .get(dir_index)
            .ok_or_else(|| no_entry(dir_index))?;
        let span = self.entry_span(dir_index)?;
        let (bytes, pinned) = self.read_span_pinned(span.start, span.end)?;
        let wrap = |e| self.entry_error(dir_index, e);
        // The inflation of a gzip entry is the open's work (`detect`'s
        // "Work"); its refusal is the open's limit, as a block decode's is.
        let ty = check_entry_type(&bytes, &self.ctx.limits, self.meter()).map_err(|d| {
            wrap(match d {
                DetectError::Work(l) => Error::Limit(l),
                d => Error::Detect(d),
            })
        })?;
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
            pinned: pinned.max(u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
        };
        // A failure the cell keeps: its grants are the open's work (module
        // docs, "Memory and work").
        let meter = self.meter();
        let scope = GrantScope::enter(meter);
        let got = parse_file(
            e.path.clone(),
            bytes,
            ty,
            Some(parent),
            meter,
            self.ctx.file(),
        );
        let granted = scope.finish(meter);
        if got.as_ref().is_err_and(|e| !e.is_transient()) {
            meter.add_work(granted);
        }
        got.map_err(wrap)
    }
}

fn no_entry(i: usize) -> Error {
    Error::Malformed {
        what: "bundle index",
        detail: format!("no row {i}").into(),
    }
}

impl BlockInfo {
    /// A block's `u16` flags.
    fn flags_u16(&self) -> u16 {
        u16::try_from(self.flags & 0xFFFF).unwrap_or(0)
    }
}
