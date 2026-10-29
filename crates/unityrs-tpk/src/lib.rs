// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! A TPK (a type-tree package, AssetRipper's format, which UnityPy ships as
//! `resources/lzma.tpk`) loaded from a path, as unityrs-core's
//! [`TypeTreeProvider`] for objects whose file stores no typetree
//! (`docs/plans/core.md` section 3.12).
//!
//! **What UnityPy does** (`helpers/Tpk.py`), and so what [`TpkProvider`] does:
//! - `get_typetree_node(class_id, version)` (`:42-55`) looks the class up
//!   (`ClassInformation[class_id]`: a `KeyError` for a class the TPK does not
//!   hold), picks its entry for the version with `get_item_for_version`
//!   (`:472-481`: a forward scan over the entries in their stored order that
//!   keeps the last entry whose version is at or below the request and stops
//!   at the first above it; a `ValueError` when it keeps nothing or keeps an
//!   entry that is `None`, the class removed at that version), and builds
//!   that entry's tree;
//! - `generate_node` (`:58-88`) asserts the entry has a release root
//!   (`AssertionError` otherwise; an editor root does not count) and walks
//!   the node buffer from it, pre-order, each node's level its parent's plus
//!   one, each row's `m_Index` its position, `m_TypeFlags` left unset; one
//!   tree per entry (`NODES_CACHE`), shared by every version that picks it.
//!
//! - `get_common_strings()` (`helpers/TypeTreeNode.py:308-328`), which every
//!   blob typetree reads, is the TPK's common strings, every index of
//!   `StringBufferIndices` (the version list unused), each numbered by the
//!   code points before it plus one per NUL: the provider's
//!   [`TypeTreeProvider::common_strings`], which the core reads in place of
//!   its own MIT table. An index past the string buffer, on which UnityPy
//!   raises `IndexError` in every blob parse, is refused at load
//!   ([`TpkError::CommonStringPastBuffer`]).
//!
//! Each of the three exceptions is "no tree": [`TypeTreeProvider::tree`]
//! answers `Ok(None)`, which the dump records as `no_typetree`. Anything else
//! UnityPy's walk raises (a node or string id past its table: `IndexError`)
//! is a [`ProviderError`]. Versions compare as UnityPy's integers do
//! (`UnityVersion` is an `int`): a TPK's versions are its stored `u64`s
//! ([`UnityVersion::from_packed`]), and the request's is the file's.
//!
//! **Loading** follows `TpkFile` and `GetDataBlob` (`Tpk.py:146-187`): the
//! 20-byte header (`<IbbbbIII`: the sizes are unsigned), the compressed
//! bytes, then the payload decoded as UnityPy decodes it, with unityrs-core's
//! codecs: LZ4 is `lz4.block.decompress(data, uncompressed_size)` with
//! liblz4 1.9.4's acceptance rules and the size a maximum
//! ([`unityrs_core::codec::Lz4`]), LZMA is `decompress_lzma`, a raw LZMA1
//! decoder run to the end of the stream with the header's size ignored
//! ([`unityrs_core::codec::Lzma`]), and brotli is refused
//! ([`TpkError::Unsupported`]: no brotli decoder is linked). The blob is
//! then read by the fork's reader.
//!
//! **Limits, where UnityPy has none.** Loading is bounded before it
//! allocates ([`TpkError::TooLarge`]): the file at
//! [`TpkProvider::MAX_FILE_BYTES`], the decoded payload at
//! [`TpkProvider::MAX_DECODED_BYTES`], what the decoder holds (its output and
//! an LZMA dictionary, charged to a meter) at
//! [`TpkProvider::MAX_DECODER_MEMORY`], and each table of the blob at
//! [`TpkProvider::MAX_RECORDS`] records (fork patch F8). The walk stops with
//! an error at a level of [`Limits::max_tree_depth`]
//! ([`unityrs_core::LimitKind::TreeDepth`]) or past
//! [`TpkProvider::MAX_TREE_ROWS`] rows, the rows still to walk counted
//! ([`unityrs_core::LimitKind::Count`]), so a node that lists itself or an
//! ancestor, on which UnityPy's walk never ends, is an error; a tree deeper
//! than the limit, which UnityPy builds, is one too (a safety divergence,
//! plan section 2.4's F3). Each is a [`ProviderError`] whose `limit` names
//! the bound (fork patch F9).
//!
//! **Memory.** The TPK is the provider's, not an input's: its rows and trees
//! are built on the provider's own unlimited meter, once per entry, and kept
//! (plan section 3.15's constant). Each tree is computed with no lock held
//! and published through [`unityrs_core::once::get_or_publish`].
//!
//! The blob's parser is rabex 0.2.1's, vendored as `unityrs-rabex` with the
//! patches in its `UPSTREAM.md`; this crate uses only its bounded blob reader
//! (F8) and the F4 accessor (`class_entries`, `release_rows`), never rabex's
//! file reader and codecs, version, class id or node types (`clippy.toml`
//! here disallows them).

#![forbid(unsafe_code)]
// No panics on a TPK's contents (plan section 3.15). Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use rabex::tpk::{Error as BlobError, TpkNodeRow, TpkTypeTreeBlob, UnityClass};
use unityrs_core::budget::{DecodeBuf, Reservation, Unbounded};
use unityrs_core::codec::{self, BlockCodec, Codec};
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::once::get_or_publish;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::{NodeList, NodeRow, Tree, TreeError};
use unityrs_core::{Bytes, LimitError, LimitKind, RawStr, UnityVersion};

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16).
const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Why a TPK did not load.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TpkError {
    /// The file could not be read.
    #[error("reading the TPK: {0}")]
    Io(#[from] std::io::Error),
    /// Its header or its blob is malformed, where UnityPy raises too: a bad
    /// magic, version, compression or data type, compressed bytes past the
    /// end, a count or a string past the end of the payload, invalid UTF-8.
    #[error("parsing the TPK: {0}")]
    Malformed(Box<str>),
    /// Its payload did not decode, where UnityPy's codec raises too.
    #[error("decompressing the TPK ({codec}): {detail}")]
    Decompress {
        /// `"lz4"` or `"lzma"`.
        codec: &'static str,
        /// The codec's message.
        detail: Box<str>,
    },
    /// Its compression is one UnityPy decodes and unityrs does not
    /// (brotli: `PARITY.md` section 19.3).
    #[error("the TPK's {0} compression is not supported")]
    Unsupported(&'static str),
    /// It passes one of the provider's bounds (module docs), which UnityPy
    /// does not have: `what` names it (`"file"`, `"decoded payload"`,
    /// `"decoder memory"`, or the blob's table: `"versions"`, `"classes"`,
    /// `"class entries"`, `"common-string versions"`,
    /// `"common-string indices"`, `"nodes"`, `"strings"`).
    #[error("the TPK's {what}: {value} > {max}, the provider's bound")]
    TooLarge {
        /// The bound.
        what: &'static str,
        /// What the TPK has, or what it reached when it was refused.
        value: u64,
        /// The bound's value.
        max: u64,
    },
    /// It holds something other than type trees (UnityPy's `init` asserts a
    /// `TpkTypeTreeBlob`).
    #[error("the TPK holds no type trees")]
    NotTypeTrees,
    /// A common string names a string past the string buffer. UnityPy loads
    /// such a TPK and raises `IndexError` in every blob typetree parse after
    /// (`get_common_strings()`); unityrs refuses it here (`PARITY.md`
    /// section 19.3).
    #[error("the TPK's common string {at} names string {index}, past its {strings} strings")]
    CommonStringPastBuffer {
        /// The common string's position in `StringBufferIndices`.
        at: usize,
        /// The string it names.
        index: u16,
        /// The string buffer's length.
        strings: usize,
    },
}

const _: () = send_sync::<TpkError>();

/// What a picked entry resolved to, kept once per entry.
type Outcome<T> = Result<Option<Arc<T>>, ProviderError>;

/// One entry of a class: its version, and its class or `None` (the class
/// removed at that version), with the node list and the tree it gives,
/// each computed once.
struct Entry {
    version: UnityVersion,
    class: Option<UnityClass>,
    rows: OnceLock<Outcome<NodeList>>,
    tree: OnceLock<Outcome<Tree>>,
}

/// A TPK's class trees, as UnityPy's `helpers/Tpk.py` builds them (module
/// docs).
pub struct TpkProvider {
    blob: TpkTypeTreeBlob,
    /// Every class's entries, in their stored order.
    classes: HashMap<i32, Box<[Entry]>>,
    /// The blob's version list, as stored.
    versions: Box<[UnityVersion]>,
    /// The string buffer, each string once, shared by the rows that name it.
    strings: HashMap<Box<str>, RawStr>,
    /// UnityPy's `get_common_strings()` over this TPK.
    common: CommonStrings,
    limits: Limits,
    meter: Arc<Meter>,
}

const _: () = send_sync::<TpkProvider>();

impl fmt::Debug for TpkProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TpkProvider")
            .field("classes", &self.classes.len())
            .field("versions", &self.versions.len())
            .finish_non_exhaustive()
    }
}

/// UnityPy's `get_item_for_version` (`helpers/Tpk.py:472-481`): the last
/// entry at or below `version`, scanning forward and stopping at the first
/// above it. `None` entries count; the caller treats a pick of one as none.
fn pick<'e>(entries: &'e [Entry], version: &UnityVersion) -> Option<&'e Entry> {
    let mut picked = None;
    for entry in entries {
        if *version >= entry.version {
            picked = Some(entry);
        } else {
            break;
        }
    }
    picked
}

impl TpkProvider {
    /// The most rows one class tree may have. UnityPy's own TPK's largest
    /// has 3,849 (`tools/fixtures/tpk.py --survey`); a tree past this is an
    /// error, which bounds the walk of a node that lists an ancestor.
    pub const MAX_TREE_ROWS: usize = 1 << 16;

    /// The largest TPK file the provider reads, 16 MiB. UnityPy 1.25.2's
    /// own is 187,318 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_FILE_BYTES: u64 = 16 << 20;

    /// The most a TPK's payload may decode to, 16 MiB. UnityPy 1.25.2's
    /// own decodes to 1,385,610 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_DECODED_BYTES: u64 = 16 << 20;

    /// The most the decoder may hold while it decodes, 64 MiB: its output
    /// and, for LZMA, the dictionary, which unityrs-core's codec keeps within
    /// twice the output, so the output's bound is the one a stream meets
    /// first.
    pub const MAX_DECODER_MEMORY: u64 = 64 << 20;

    /// The most records any one table of the blob may hold (fork patch F8),
    /// 65,536: the most nodes and strings a 16-bit id can name. UnityPy
    /// 1.25.2's own TPK's largest table is its 22,818 nodes
    /// (`tools/fixtures/tpk.py --survey`).
    pub const MAX_RECORDS: usize = 1 << 16;

    /// The provider's name, the dump's `l2.source`.
    pub const NAME: &'static str = "tpk";

    /// Loads the TPK at `path`, with the default [`Limits`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TpkError> {
        Self::open_with(path, &Limits::default())
    }

    /// Loads the TPK at `path`; trees deeper than `limits.max_tree_depth`
    /// are errors. A file past [`Self::MAX_FILE_BYTES`] is refused unread,
    /// and one that is not a regular file (whose length is not known) is
    /// read only that far.
    pub fn open_with(path: impl AsRef<Path>, limits: &Limits) -> Result<Self, TpkError> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let too_large = |value: u64| TpkError::TooLarge {
            what: "file",
            value,
            max: Self::MAX_FILE_BYTES,
        };
        if len > Self::MAX_FILE_BYTES {
            return Err(too_large(len));
        }
        let mut bytes = Vec::new();
        file.take(Self::MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if read > Self::MAX_FILE_BYTES {
            return Err(too_large(read));
        }
        Self::from_bytes(&bytes, limits)
    }

    /// Loads a TPK from its bytes (as UnityPy's `TpkFile(stream).GetDataBlob()`
    /// reads them, `helpers/Tpk.py:36-39`; module docs).
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self, TpkError> {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if len > Self::MAX_FILE_BYTES {
            return Err(TpkError::TooLarge {
                what: "file",
                value: len,
                max: Self::MAX_FILE_BYTES,
            });
        }
        let payload = payload(bytes)?;
        let blob = TpkTypeTreeBlob::from_reader_bounded(&mut &payload[..], Self::MAX_RECORDS)
            .map_err(|e| match e {
                BlobError::TooMany { table, count, max } => TpkError::TooLarge {
                    what: table,
                    value: u64::try_from(count).unwrap_or(u64::MAX),
                    max: u64::try_from(max).unwrap_or(u64::MAX),
                },
                e => TpkError::Malformed(e.to_string().into()),
            })?;
        drop(payload);
        let mut classes = HashMap::new();
        for id in blob.class_ids() {
            let entries = blob.class_entries(id).into_iter().flatten();
            let entries = entries
                .map(|(version, class)| Entry {
                    version: UnityVersion::from_packed(version),
                    class: class.cloned(),
                    rows: OnceLock::new(),
                    tree: OnceLock::new(),
                })
                .collect();
            classes.insert(id, entries);
        }
        let versions = blob
            .versions
            .iter()
            .map(|v| UnityVersion::from_packed(v.0))
            .collect();
        let strings: HashMap<Box<str>, RawStr> = blob
            .string_buffer
            .iter()
            .map(|s| (Box::from(s.as_str()), RawStr::from(s.as_str())))
            .collect();
        let common = common_strings(&blob, &strings)?;
        Ok(Self {
            blob,
            classes,
            versions,
            strings,
            common,
            limits: *limits,
            meter: Meter::unlimited(),
        })
    }

    /// The number of classes the TPK holds (UnityPy's
    /// `len(ClassInformation)`; a class listed twice counts once, its last
    /// listing kept, as a Python dict keeps it).
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// The classes the TPK holds, sorted.
    #[must_use]
    pub fn class_ids(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self.classes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// The TPK's version list, as stored (UnityPy's `Versions`).
    #[must_use]
    pub fn versions(&self) -> &[UnityVersion] {
        &self.versions
    }

    /// The node list UnityPy builds for `class_id` at `version`, before it
    /// is compiled: `Ok(None)` wherever UnityPy raises `KeyError`,
    /// `ValueError` or `AssertionError` (module docs). Every version that
    /// picks one entry gets the same `Arc`.
    pub fn node_list(
        &self,
        class_id: i32,
        version: &UnityVersion,
    ) -> Result<Option<Arc<NodeList>>, ProviderError> {
        let Some(entry) = self.entry(class_id, version) else {
            return Ok(None);
        };
        self.rows_of(entry).clone()
    }

    /// The picked entry when it has a class (`Tpk.py:49-51`).
    fn entry(&self, class_id: i32, version: &UnityVersion) -> Option<&Entry> {
        let entries = self.classes.get(&class_id)?;
        pick(entries, version).filter(|e| e.class.is_some())
    }

    fn rows_of<'e>(&self, entry: &'e Entry) -> &'e Outcome<NodeList> {
        get_or_publish(&entry.rows, || self.walk(entry.class.as_ref()))
    }

    /// `generate_node` (`Tpk.py:58-88`) for one entry's class.
    fn walk(&self, class: Option<&UnityClass>) -> Outcome<NodeList> {
        let Some(class) = class else {
            return Ok(None);
        };
        let max_depth = usize::try_from(self.limits.max_tree_depth).unwrap_or(usize::MAX);
        let rows = self
            .blob
            .release_rows(class, max_depth, Self::MAX_TREE_ROWS)
            .map_err(|e| match e {
                BlobError::WalkDepth { depth, max } => limit(LimitKind::TreeDepth, depth, max),
                BlobError::WalkRows { rows, max } => limit(LimitKind::Count, rows, max),
                e => error(&e),
            })?;
        let Some(rows) = rows else {
            // `assert class_info.ReleaseRootNode is not None` (`Tpk.py:64`).
            return Ok(None);
        };
        let rows: Vec<NodeRow> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.node_row(index, row))
            .collect();
        let rows = MeteredVec::from_vec(rows, &self.meter).map_err(|e| error(&e))?;
        NodeList::from_rows(rows)
            .map(|list| Some(Arc::new(list)))
            .map_err(|e| error(&e))
    }

    /// One row as UnityPy's `TypeTreeNode(...)` call makes it
    /// (`Tpk.py:73-83`): `m_Index` the row's position, `m_TypeFlags`,
    /// `m_VariableCount` and `m_RefTypeHash` unset.
    fn node_row(&self, index: usize, row: &TpkNodeRow<'_>) -> NodeRow {
        let string = |s: &str| {
            self.strings
                .get(s)
                .cloned()
                .unwrap_or_else(|| RawStr::from(s))
        };
        NodeRow {
            level: row.level,
            ty: string(row.type_name),
            name: string(row.name),
            byte_size: row.byte_size,
            version: i32::from(row.version),
            type_flags: None,
            variable_count: None,
            // Below `MAX_TREE_ROWS`.
            index: Some(i32::try_from(index).unwrap_or(i32::MAX)),
            // UnityPy keeps the stored `u32` (`Tpk.py:361`), zero-extended.
            meta_flag: Some(i64::from(row.meta_flag)),
            ref_type_hash: None,
        }
    }
}

fn error(e: &dyn fmt::Display) -> ProviderError {
    ProviderError {
        provider: TpkProvider::NAME,
        detail: e.to_string().into_boxed_str(),
        limit: None,
    }
}

/// A walk that crossed a bound, as a limit error (fork patch F9).
fn limit(kind: LimitKind, value: usize, max: usize) -> ProviderError {
    let limit = LimitError {
        kind,
        value: u64::try_from(value).unwrap_or(u64::MAX),
        max: u64::try_from(max).unwrap_or(u64::MAX),
    };
    ProviderError {
        limit: Some(limit),
        ..error(&limit)
    }
}

/// UnityPy's `TpkFile` and `GetDataBlob` up to the blob (`Tpk.py:146-187`):
/// the header checked in UnityPy's order, then the payload, decoded.
fn payload(bytes: &[u8]) -> Result<Bytes, TpkError> {
    let bad = |what: &str| TpkError::Malformed(what.into());
    // `TpkFile.Struct.unpack(stream.read(20))`: `<IbbbbIII`.
    let Some(head) = bytes.get(..20) else {
        return Err(bad("struct.error: unpack requires a buffer of 20 bytes"));
    };
    let u32_at = |at: usize| {
        head.get(at..at + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, u32::from_le_bytes)
    };
    let byte = |at: usize| head.get(at).copied().unwrap_or(0);
    if u32_at(0) != 0x2A4B_5054 {
        return Err(bad("Invalid TPK magic bytes"));
    }
    if byte(4) != 1 {
        return Err(bad("Invalid TPK version number"));
    }
    // `TpkCompressionType(...)` and `TpkDataType(...)`: the signed bytes must
    // be members (0-3 and 0-5).
    let compression = byte(5);
    if compression > 3 {
        return Err(bad("ValueError: not a TpkCompressionType"));
    }
    let data_type = byte(6);
    if data_type > 5 {
        return Err(bad("ValueError: not a TpkDataType"));
    }
    let (compressed, uncompressed) = (u32_at(12), u32_at(16));
    // `stream.read(self.CompressedSize)`, which must return them all.
    let data = usize::try_from(compressed)
        .ok()
        .and_then(|n| bytes.get(20..20usize.checked_add(n)?))
        .ok_or_else(|| bad("Invalid compressed size"))?;
    let data = Bytes::copy_from_slice(data);
    let decoded = match compression {
        0 => data,
        1 => decode(Codec::Lz4, &codec::Lz4, &data, uncompressed)?,
        2 => decode(Codec::Lzma, &codec::Lzma, &data, uncompressed)?,
        _ => return Err(TpkError::Unsupported("brotli")),
    };
    // `self.DataType.ToBlob(...)`, then `init`'s `assert isinstance(blob,
    // TpkTypeTreeBlob)`: any other blob fails the load.
    if data_type != 0 {
        return Err(TpkError::NotTypeTrees);
    }
    Ok(decoded)
}

/// UnityPy's `get_common_strings()` (`helpers/TypeTreeNode.py:308-328`)
/// over `blob`: `GetStrings`, the string of every index in order
/// (`Tpk.py:425-426`), numbered as [`CommonStrings::new`] numbers them; each
/// string shared with the rows that name it.
fn common_strings(
    blob: &TpkTypeTreeBlob,
    strings: &HashMap<Box<str>, RawStr>,
) -> Result<CommonStrings, TpkError> {
    let indices = &blob.common_string.string_buffer_indices;
    let mut table = Vec::new();
    for (at, &index) in indices.iter().enumerate() {
        let s = blob
            .string_buffer
            .get(usize::from(index))
            .and_then(|s| strings.get(s.as_str()))
            .ok_or(TpkError::CommonStringPastBuffer {
                at,
                index,
                strings: blob.string_buffer.len(),
            })?;
        table.push(s.clone());
    }
    Ok(CommonStrings::new(table))
}

/// `data` decoded by `codec`, as one block of unityrs-core's: the output
/// capped at [`TpkProvider::MAX_DECODED_BYTES`], and the output and an LZMA
/// dictionary charged to a meter of [`TpkProvider::MAX_DECODER_MEMORY`].
fn decode(
    which: Codec,
    codec: &dyn BlockCodec,
    data: &Bytes,
    uncompressed: u32,
) -> Result<Bytes, TpkError> {
    let name = if which == Codec::Lz4 { "lz4" } else { "lzma" };
    let fail = |e: unityrs_core::Error| match e {
        unityrs_core::Error::Limit(l) => TpkError::TooLarge {
            what: if l.kind == LimitKind::InputMemory {
                "decoder memory"
            } else {
                "decoded payload"
            },
            value: l.value,
            max: l.max,
        },
        unityrs_core::Error::Decompress { detail, .. } => TpkError::Decompress {
            codec: name,
            detail: Box::from(&*detail),
        },
        e => TpkError::Decompress {
            codec: name,
            detail: e.to_string().into(),
        },
    };
    let meter = Meter::new(
        TpkProvider::MAX_DECODER_MEMORY,
        Arc::new(Unbounded),
        Reservation::Input,
    );
    let cap = TpkProvider::MAX_DECODED_BYTES;
    let mut out =
        DecodeBuf::new(&meter, 0, cap, LimitKind::BlockOutput).map_err(|e| fail(e.into()))?;
    match codec::decode_block(which, codec, data, uncompressed, &mut out, cap).map_err(fail)? {
        Some(input) => Ok(input),
        None => out.into_bytes().map_err(|e| fail(e.into())),
    }
}

impl TypeTreeProvider for TpkProvider {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// The TPK's common strings (UnityPy's `get_common_strings()`, module
    /// docs).
    fn common_strings(&self) -> Option<&CommonStrings> {
        Some(&self.common)
    }

    /// [`TpkProvider::node_list`], compiled once per entry with
    /// [`Tree::compile`] on the provider's meter and kept: every request
    /// that picks one entry gets the same `Arc` (UnityPy's `NODES_CACHE`).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some(entry) = self.entry(req.class_id, req.version) else {
            return Ok(None);
        };
        get_or_publish(&entry.tree, || {
            let Some(list) = self.rows_of(entry).clone()? else {
                return Ok(None);
            };
            Tree::compile(list, &self.limits)
                .map(Some)
                .map_err(|e| match e {
                    TreeError::Limit(l) => ProviderError {
                        limit: Some(l),
                        ..error(&l)
                    },
                    e => error(&e),
                })
        })
        .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every public type is asserted `Send + Sync` by its full path (plan
    /// 3.16), as in unityrs-core.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }

    /// Plan 4.1: no thread, no wait; a provider is called from a caller's
    /// thread, maybe a pool's.
    #[test]
    fn the_provider_starts_no_thread_and_never_waits() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "waits or starts a thread: {found:?}");
    }

    /// Plan 3.16: each entry's rows and tree are computed outside their
    /// cells and published through `once::get_or_publish`.
    #[test]
    fn every_cell_is_filled_through_the_once_helper() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::cell_fills(&src);
        assert!(
            found.is_empty(),
            "a cell filled outside the once helper: {found:?}"
        );
    }
}
