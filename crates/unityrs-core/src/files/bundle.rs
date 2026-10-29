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
//! `LimitKind::InputMemory` instead of growing; every byte decoded also
//! counts as the input's decode work (`LimitKind::DecodeWork`), which is
//! never returned. The header, blocks, directory, `files` and the per-row
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

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::ops::Range;
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use crate::error::Error;
use crate::files::{File, Parent};
use crate::limits::{Charge, Meter, MeteredVec};
use crate::once::ResultCell;
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::version::{FlagsClass, ResolvedVersion, VersionError};

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
    /// parse cells), charged to the input at open.
    charge: Charge,
    ctx: Arc<FileContext>,
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
        todo!()
    }

    /// The meter of the input it belongs to; its entries charge it too.
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The input bytes it was opened on (for telling two handles on one
    /// input apart from two inputs: a resource read through `from`'s own
    /// handle, plan section 3.13).
    pub fn source(&self) -> &Bytes {
        &self.source
    }

    /// The name it was opened under.
    pub fn name(&self) -> &RawStr {
        &self.name
    }

    /// The header.
    pub fn header(&self) -> &BundleHeader {
        &self.header
    }

    /// The blocks, in file order (`Fs` layout).
    pub fn blocks(&self) -> &[BlockInfo] {
        &self.blocks
    }

    /// The directory, in file order, repeated paths kept.
    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }

    /// UnityPy's `files` dict, in order.
    pub fn files(&self) -> &[FileSlot] {
        &self.files
    }

    /// The container this bundle is an entry of.
    pub fn parent(&self) -> Option<&Parent> {
        self.parent.as_ref()
    }

    /// The context it was opened with.
    pub fn context(&self) -> &Arc<FileContext> {
        &self.ctx
    }

    /// The sum of the blocks' *declared* decoded sizes: what a full decode is
    /// expected to cost. The actual total can differ (module docs).
    pub fn uncompressed_size(&self) -> u64 {
        todo!()
    }

    /// Directory entry `dir_index`'s bytes: the Python slice
    /// `data[offset : offset + size]` of the joined data (size -1 to the end,
    /// negative indexes from the end, short at the end). Measures every block
    /// before the slice first (module docs).
    pub fn entry_bytes(&self, dir_index: usize) -> Result<Bytes, Error> {
        todo!()
    }

    /// A byte range of an entry (a `.resS` read), short at the end of the
    /// entry as a Python slice. It measures every block before the range as
    /// any read does, slices a cached run that covers the range's blocks, and
    /// otherwise decodes those blocks into a run it caches, so later ranges
    /// in the same blocks are slices of it.
    pub fn entry_range(&self, dir_index: usize, range: Range<u64>) -> Result<Bytes, Error> {
        todo!()
    }

    /// Directory entry `dir_index`, detected and parsed (`parse_file`) and
    /// published once ([`crate::once::get_or_publish_result`]); under
    /// contention two callers may both parse it, and the loser's result (and
    /// its charge) is dropped. A parse the meter or the budget refuses is
    /// returned and not kept.
    pub fn entry(&self, dir_index: usize) -> Result<File, Error> {
        todo!()
    }

    /// UnityPy's eager load (`File.read_files`, `File.py:76-90`): every block
    /// decoded, every directory entry parsed in order (a path repeated in the
    /// directory included: its earlier entry can fail the bundle though the
    /// `files` dict keeps the later one); the first failure fails the bundle.
    pub fn load_all(&self) -> Result<(), Error> {
        todo!()
    }
}
