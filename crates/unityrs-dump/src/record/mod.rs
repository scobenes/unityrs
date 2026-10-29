//! The records of `docs/CANONICAL_DUMP.md` section 4, as plain data, and
//! their canonical lines.
//!
//! Each record type mirrors one of `tools/parity/unitypy_dump.py`'s record
//! builders, member for member and in its order (`canonical.py`
//! `RECORD_MEMBERS`, `FILE_L1_MEMBERS`, `TYPE_MEMBERS`, `OBJECT_SECTIONS`):
//! the `dump` header, a `file` record (its `l0`, an `l1` by kind, an
//! `error`), an `object` record (`l1`, `l2`, `l3`) and the `end` record.
//! [`crate::input`] fills them from `unityrs-core`'s public API; a test
//! fills them by hand. The data holds what was read (raw names as
//! [`RawStr`], packed versions exactly), and the writers apply the
//! format's rules: the metadata-string rule to every name, hex to fixed
//! bytes, `null` where a member does not apply.

use std::sync::Arc;

use unityrs_core::RawStr;
use unityrs_core::detect::{FileType, ParsedAs};
use unityrs_core::typetree::NodeList;
use unityrs_core::version::{FlagsClass, Packed};
use unityrs_core::walk::L2Kind;

mod line;
#[cfg(test)]
mod tests;

pub use line::{end, file_line, header, object_line};

/// The format's name and version (section 4.1).
pub const FORMAT: &str = "unityrs-canonical-dump";
/// The format version this crate writes.
pub const FORMAT_VERSION: u32 = 1;

/// The longest note, in characters (`unitypy_dump.py` `NOTE_MAX`).
pub const NOTE_MAX: usize = 300;

/// A note: free text, never compared, cut to [`NOTE_MAX`] characters as
/// `unitypy_dump.py`'s `note` cuts it.
#[must_use]
pub fn note(text: &str) -> String {
    if text.chars().count() <= NOTE_MAX {
        return text.to_owned();
    }
    let mut s: String = text.chars().take(NOTE_MAX - 3).collect();
    s.push_str("...");
    s
}

/// `l0` (section 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L0 {
    /// `check_file_type`'s verdict, or `None` when it raised (`"error"`).
    pub ty: Option<FileType>,
    /// What was built.
    pub parsed_as: ParsedAs,
    /// The bytes `ty` was decided on.
    pub size: u64,
    /// Their sha256.
    pub sha256: [u8; 32],
}

const _: () = crate::send_sync::<L0>();

/// The `FileType` name (`l0.type`).
#[must_use]
pub fn file_type_name(ty: Option<FileType>) -> &'static str {
    match ty {
        Some(FileType::AssetsFile) => "AssetsFile",
        Some(FileType::BundleFile) => "BundleFile",
        Some(FileType::WebFile) => "WebFile",
        Some(FileType::ResourceFile) => "ResourceFile",
        Some(FileType::Zip) => "ZIP",
        None => "error",
    }
}

/// The `l0.parsed_as` name.
#[must_use]
pub fn parsed_as_name(p: ParsedAs) -> &'static str {
    match p {
        ParsedAs::SerializedFile => "SerializedFile",
        ParsedAs::BundleFile => "BundleFile",
        ParsedAs::WebFile => "WebFile",
        ParsedAs::Raw => "raw",
        ParsedAs::Zip => "zip",
        ParsedAs::Error => "error",
    }
}

/// A file record's `error` kind (section 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileErrorKind {
    /// `check_file_type` raised.
    Detect,
    /// The parser raised.
    Parse,
    /// The process dumping the input died of a signal (the oracle's alone).
    Crash,
}

/// A file record's `error`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    /// The kind.
    pub kind: FileErrorKind,
    /// The message, if any.
    pub note: Option<String>,
}

const _: () = crate::send_sync::<FileErrorKind>();
const _: () = crate::send_sync::<FileError>();

/// One block row: `[uncompressed, compressed, flags]`, `compressed` being
/// the bytes read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRow {
    /// Declared decoded size.
    pub uncompressed: u32,
    /// Bytes read for it.
    pub compressed: u32,
    /// Flags.
    pub flags: u32,
}

/// One directory row: `[offset, size, flags, path]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirRow {
    /// Offset in the decoded data.
    pub offset: i64,
    /// Size.
    pub size: i64,
    /// Flags, `None` outside the UnityFS layout.
    pub flags: Option<u32>,
    /// The path, as read.
    pub path: RawStr,
}

/// `l1` of a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BundleL1 {
    /// `signature`.
    pub signature: RawStr,
    /// UnityPy's `version`, the format number.
    pub format: u32,
    /// `version_player`.
    pub version_player: RawStr,
    /// `version_engine`.
    pub version_engine: RawStr,
    /// `parse_version()` packed, or `None` where it raises.
    pub engine_version_packed: Option<Packed>,
    /// The data flags (`Fs` layout).
    pub flags: Option<u32>,
    /// The flag set (`Fs` layout).
    pub flags_class: Option<FlagsClass>,
    /// Whether the 16-byte alignment applied.
    pub block_alignment: bool,
    /// Whether UnityPy built a decryptor.
    pub encrypted: bool,
    /// The web/raw hash.
    pub hash: Option<[u8; 16]>,
    /// The web/raw CRC.
    pub crc: Option<u32>,
    /// The blocks-info section's row.
    pub blocks_info: Option<BlockRow>,
    /// The blocks (`Fs` layout).
    pub blocks: Option<Vec<BlockRow>>,
    /// The directory, in file order.
    pub directory: Vec<DirRow>,
}

/// `l1` of a WebFile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebL1 {
    /// `signature`.
    pub signature: RawStr,
    /// `"none"`, `"gzip"` or `"brotli"`.
    pub packer: &'static str,
    /// The directory (flags `None`).
    pub directory: Vec<DirRow>,
}

/// The `header` of a SerializedFile's `l1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializedHeaderRow {
    /// `metadata_size`.
    pub metadata_size: u32,
    /// `file_size`.
    pub file_size: i64,
    /// `version`.
    pub version: u32,
    /// `data_offset`.
    pub data_offset: i64,
    /// `true` for big-endian (`">"`).
    pub big_endian: bool,
    /// The reserved bytes (format 9 and up).
    pub reserved: Option<[u8; 3]>,
    /// Format 22's trailing `i64`, else 0.
    pub unknown: i64,
}

/// A type record (section 4.2 "Type record").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeRecord {
    /// `class_id`.
    pub class_id: i32,
    /// `is_stripped_type`.
    pub is_stripped_type: Option<bool>,
    /// `script_type_index`.
    pub script_type_index: i16,
    /// `script_id`.
    pub script_id: Option<[u8; 16]>,
    /// `old_type_hash`.
    pub old_type_hash: Option<[u8; 16]>,
    /// The stored tree, written as its rows.
    pub nodes: Option<Arc<NodeList>>,
    /// `type_dependencies`.
    pub type_dependencies: Option<Vec<i32>>,
    /// `class_name` (ref types).
    pub class_name: Option<RawStr>,
    /// `namespace` (ref types).
    pub namespace: Option<RawStr>,
    /// `assembly` (ref types).
    pub assembly: Option<RawStr>,
}

/// One external: `[temp_empty, guid, type, path]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRow {
    /// `temp_empty`.
    pub temp_empty: Option<RawStr>,
    /// `guid`.
    pub guid: Option<[u8; 16]>,
    /// `type`.
    pub kind: Option<i32>,
    /// `path`.
    pub path: RawStr,
}

/// One container row: `[path, preload_index, preload_size, file_id, path_id]`,
/// each `None` (`null`) where UnityPy's object holds no string or no `int`
/// (`docs/CANONICAL_DUMP.md` section 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRow {
    /// The asset path.
    pub path: Option<RawStr>,
    /// `preloadIndex`.
    pub preload_index: Option<i128>,
    /// `preloadSize`.
    pub preload_size: Option<i128>,
    /// The asset's `m_FileID`.
    pub file_id: Option<i128>,
    /// The asset's `m_PathID`.
    pub path_id: Option<i128>,
}

/// `l1` of a SerializedFile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializedL1 {
    /// `header`.
    pub header: SerializedHeaderRow,
    /// `unity_version`: as read, or UnityPy's default `"2.5.0f5"` below format 7.
    pub unity_version: RawStr,
    /// `version_packed` (format 7 and up).
    pub version_packed: Option<Packed>,
    /// `target_platform` (format 8 and up).
    pub target_platform: Option<i32>,
    /// `enable_type_tree`.
    pub enable_type_tree: bool,
    /// `big_id_enabled`.
    pub big_id_enabled: i32,
    /// `types`.
    pub types: Vec<TypeRecord>,
    /// `object_order`: path ids in dict order.
    pub object_order: Vec<i64>,
    /// `script_types`: `[file_index, identifier]`.
    pub script_types: Vec<(i32, i64)>,
    /// `externals`.
    pub externals: Vec<ExternalRow>,
    /// `ref_types` (format 20 and up).
    pub ref_types: Option<Vec<TypeRecord>>,
    /// `user_information` (format 5 and up).
    pub user_information: Option<RawStr>,
    /// `container`.
    pub container: Vec<ContainerRow>,
}

/// A file record's `l1`, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileL1 {
    /// `kind: "bundle"`.
    Bundle(BundleL1),
    /// `kind: "web"`.
    Web(WebL1),
    /// `kind: "serialized"`.
    Serialized(Box<SerializedL1>),
}

const _: () = crate::send_sync::<BlockRow>();
const _: () = crate::send_sync::<DirRow>();
const _: () = crate::send_sync::<BundleL1>();
const _: () = crate::send_sync::<WebL1>();
const _: () = crate::send_sync::<SerializedHeaderRow>();
const _: () = crate::send_sync::<TypeRecord>();
const _: () = crate::send_sync::<ExternalRow>();
const _: () = crate::send_sync::<ContainerRow>();
const _: () = crate::send_sync::<SerializedL1>();
const _: () = crate::send_sync::<FileL1>();

/// A `file` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    /// The identity: `[input]`, `[input, entry]`, ...
    pub file: Vec<RawStr>,
    /// Detection.
    pub l0: L0,
    /// Container metadata, `None` without L1 or for raw, zip and error.
    pub l1: Option<FileL1>,
    /// Only an input's own record carries one.
    pub error: Option<FileError>,
}

/// An object's `l1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectL1 {
    /// `type_id`.
    pub type_id: i32,
    /// `class_id`.
    pub class_id: i32,
    /// `byte_start`, absolute within the SerializedFile.
    pub byte_start: i128,
    /// `byte_size`.
    pub byte_size: u32,
    /// Below format 11.
    pub is_destroyed: Option<u16>,
    /// Formats 15 and 16.
    pub is_stripped: Option<i8>,
}

/// An object's `l2.error`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L2Error {
    /// The kind.
    pub kind: L2Kind,
    /// Where the read ended, for `check_read` only.
    pub bytes_read: Option<i64>,
    /// The message, if any.
    pub note: Option<String>,
}

/// An object's `l2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L2 {
    /// The tree root's `m_Type`, or `None` without a tree.
    pub root_type: Option<RawStr>,
    /// `"embedded"` or `"tpk"` (a provider's name), or `None` without a tree.
    pub source: Option<&'static str>,
    /// `None` on success.
    pub error: Option<L2Error>,
    /// The encoded value (canonical JSON text), on success or `check_read`.
    pub value: Option<String>,
}

/// An object's resave outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resave {
    /// The writer returned the raw bytes.
    Equal,
    /// The writer returned other bytes.
    Differs {
        /// Their length.
        size: u64,
        /// Their sha256.
        sha256: [u8; 32],
        /// The first differing offset, or the shorter length.
        first_diff: u64,
    },
    /// The writer raised.
    Error {
        /// The message, if any.
        note: Option<String>,
    },
    /// `l2` has an error, so no write was attempted.
    Skipped,
}

/// An object's `l3`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3 {
    /// `len(get_raw_data())`.
    pub raw_size: u64,
    /// Its sha256.
    pub raw_sha256: [u8; 32],
    /// The resave outcome.
    pub resave: Resave,
}

/// An `object` record (its `file` is the owning SerializedFile's identity,
/// passed to [`object_line`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectRecord {
    /// `path_id`.
    pub path_id: i64,
    /// `None` without L1.
    pub l1: Option<ObjectL1>,
    /// `None` without L2.
    pub l2: Option<L2>,
    /// `None` without L3.
    pub l3: Option<L3>,
}

const _: () = crate::send_sync::<FileRecord>();
const _: () = crate::send_sync::<ObjectL1>();
const _: () = crate::send_sync::<L2Error>();
const _: () = crate::send_sync::<L2>();
const _: () = crate::send_sync::<Resave>();
const _: () = crate::send_sync::<L3>();
const _: () = crate::send_sync::<ObjectRecord>();

/// The first offset where `a` and `b` differ, or the shorter length
/// (`unitypy_dump.py` `first_difference`).
#[must_use]
pub fn first_difference(a: &[u8], b: &[u8]) -> u64 {
    let n = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    u64::try_from(n).unwrap_or(u64::MAX)
}
