//! The file-level error type.
//!
//! Object-level outcomes (the canonical dump's L2 kinds) are
//! [`crate::walk::ParseError`]; the writer's are [`crate::walk::WriteError`].
//! `Error` is `Clone` because single-flight caches (`OnceLock`) hand the same
//! result to every waiter.
//!
//! Owner: the scaffold (frozen). A package that needs a new variant asks the
//! integration package; `#[non_exhaustive]` keeps that additive.

use std::sync::Arc;

use crate::budget::BudgetExceeded;
use crate::codec::Codec;
use crate::detect::DetectError;
use crate::raw::RawStr;
use crate::typetree::TreeError;
use crate::version::VersionError;
use crate::walk::ParseError;

/// Why loading a file failed. Never a panic.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `check_file_type` raised (the dump's `"detect"`).
    #[error("detection: {0}")]
    Detect(#[from] DetectError),
    /// A read past the end where UnityPy raises too (`struct.error`, `EOFError`).
    #[error("{what}: out of bounds at {pos} (need {need}, have {have})")]
    OutOfBounds {
        /// What was being read.
        what: &'static str,
        /// Offset of the read.
        pos: u64,
        /// Bytes needed.
        need: u64,
        /// Bytes left.
        have: u64,
    },
    /// Input UnityPy rejects with an exception other than a short read.
    #[error("malformed {what}: {detail}")]
    Malformed {
        /// What was being read.
        what: &'static str,
        /// The detail, for people.
        detail: Box<str>,
    },
    /// A format or feature unityrs does not read (yet), each named.
    #[error("unsupported: {0}")]
    Unsupported(Unsupported),
    /// A block failed to decode.
    #[error("{codec:?} block: {detail}")]
    Decompress {
        /// The block's codec.
        codec: Codec,
        /// The decoder's message.
        detail: Box<str>,
    },
    /// A version string did not parse, or no fallback was set.
    #[error(transparent)]
    Version(#[from] VersionError),
    /// From format 16, an object's `type_id` outside `types` (Python indexing:
    /// negative counts from the end; `ObjectReader.py:98-101`).
    #[error("type index {index} out of range ({len} types)")]
    TypeIndex {
        /// The object's `type_id`.
        index: i32,
        /// How many types the file has.
        len: usize,
    },
    /// A typetree's rows could not be built into a tree, or the tree is over a limit.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// The AssetBundle object read at load failed, which fails the file
    /// (`SerializedFile.py:316-324`; `CANONICAL_DUMP.md` section 7).
    #[error("AssetBundle: {0}")]
    AssetBundle(Box<ParseError>),
    /// A bundle entry failed, which fails the bundle (`File.py:76-90`).
    #[error("entry {name}: {source}")]
    Entry {
        /// The entry's directory path.
        name: RawStr,
        /// Why it failed.
        source: Box<Error>,
    },
    /// A bound from [`crate::limits::Limits`] was exceeded.
    #[error(transparent)]
    Limit(#[from] LimitError),
    /// The memory budget refused a reservation.
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
    /// A CAB, external, object or resource that is not there.
    #[error("not found: {0}")]
    NotFound(NotFound),
    /// An I/O error from a source.
    #[error("io: {0}")]
    Io(Arc<std::io::Error>),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(Arc::new(e))
    }
}

/// Formats and features that are named and refused, never guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unsupported {
    /// The `UnityArchive` signature; UnityPy raises too (`BundleFile.py:41`).
    #[error("UnityArchive bundles")]
    UnityArchive,
    /// LZHAM blocks; UnityPy raises too (`CompressionHelper.py:234-235`).
    #[error("LZHAM compression")]
    Lzham,
    /// A compression type no codec is registered for (`BundleFile.py:515-521`).
    #[error("compression type {0}")]
    Codec(u8),
    /// UnityWeb and UnityRaw outside format 6: P3.
    #[error("the UnityWeb/UnityRaw layout")]
    WebRawLayout,
    /// `WebFile` (`UnityWebData`, `TuanjieWebData`): P3.
    #[error("WebFile")]
    WebFile,
    /// An encrypted bundle without a decryptor (UnityCN: P3).
    #[error("an encrypted bundle without a decryptor")]
    Encrypted,
    /// A file inside a zip: P3.
    #[error("zip members")]
    Zip,
}

/// Which bound was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// [`crate::limits::Limits::max_block_output`].
    BlockOutput,
    /// [`crate::limits::Limits::max_bundle_uncompressed`].
    BundleUncompressed,
    /// [`crate::limits::Limits::max_inflate`].
    Inflate,
    /// [`crate::limits::Limits::max_metadata_records`].
    MetadataRecords,
    /// [`crate::limits::Limits::max_tree_rows`].
    TreeRows,
    /// [`crate::limits::Limits::max_tree_depth`].
    TreeDepth,
    /// [`crate::limits::Limits::max_value_depth`].
    ValueDepth,
    /// [`crate::limits::Limits::max_values_per_object`].
    ValuesPerObject,
    /// [`crate::limits::Limits::max_zero_size_elements`].
    ZeroSizeElements,
    /// More nodes, types, objects or ref-type trees than an index type holds.
    Count,
}

/// A bound that was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("limit {kind:?}: {value} > {max}")]
pub struct LimitError {
    /// Which bound.
    pub kind: LimitKind,
    /// The value that crossed it.
    pub value: u64,
    /// The bound.
    pub max: u64,
}

/// What was not found.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NotFound {
    /// No CAB of that simplified name, loaded or loadable.
    #[error("CAB {0}")]
    Cab(RawStr),
    /// No resource under any of UnityPy's candidate names (`ResourceReader.py:10-34`).
    #[error("resource {0}")]
    Resource(RawStr),
    /// A source the environment has no slot for.
    #[error("source {0}")]
    Source(u32),
}
