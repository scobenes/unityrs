//! `BundleFile` (`files/BundleFile.py`), read lazily.
//!
//! [`BundleFile::open`] reads the header, blocks-info and directory and
//! decodes no data block, so a registry over thousands of bundles is cheap
//! and [`BundleFile::uncompressed_size`] (the declared total) is known before
//! any decode (the batch engine's admission).
//!
//! **One layout** (plan section 3.6). UnityPy always reads entries from the
//! `b"".join` of every block's *actual* output (`BundleFile.py:173-186`), so
//! an entry's offset depends on the actual length of every block before it.
//! Before a lazy read returns bytes ending at joined offset `X`, every block
//! starting below `X` has a known length (`lengths`): a stored block's is
//! known at open; an unmeasured block wholly before the read is decoded with
//! its output discarded; the blocks the read overlaps are decoded once and
//! cached as a run. Every entry is a zero-copy slice of its run, and a lazy
//! read equals [`BundleFile::load_all`]'s bytes on every bundle, malformed
//! ones included. An entry is the Python slice `data[offset : offset + size]`
//! of the joined data (`File.py:78-81`; [`crate::stream::py_read`]).
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
//! - LZMA's output can outgrow its declared size: the limits and the budget
//!   follow the actual bytes ([`crate::limits::Limits::max_bundle_uncompressed`]).
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
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;

use crate::error::Error;
use crate::files::{File, Parent};
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
    /// Each block's actual decoded length, once known (a stored block's at
    /// open, any other's when it is first decoded or measured).
    lengths: Box<[OnceLock<u64>]>,
    /// Decoded block runs: `(first block, end block, bytes)`.
    runs: Mutex<Vec<(usize, usize, Bytes)>>,
    /// Parsed entries, by directory index.
    entries: Box<[OnceLock<Result<File, Error>>]>,
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
    pub fn open(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        todo!()
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

    /// A byte range of an entry, decoding only the blocks it overlaps (a
    /// `.resS` read). Short at the end of the entry, as a Python slice.
    pub fn entry_range(&self, dir_index: usize, range: Range<u64>) -> Result<Bytes, Error> {
        todo!()
    }

    /// Directory entry `dir_index`, detected and parsed once (`parse_file`).
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
