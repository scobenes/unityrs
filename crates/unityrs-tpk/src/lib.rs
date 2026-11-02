// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! A TPK (a type-tree package, AssetRipper's format, which UnityPy ships as
//! `resources/lzma.tpk`) loaded from a path, as unityrs-core's
//! [`TypeTreeProvider`] for objects whose file stores no typetree
//! (`docs/plans/core.md` section 3.12).
//!
//! The crate is a port of UnityPy's `helpers/Tpk.py`, on raw bytes, one
//! module per part of it:
//! - `file`: `TpkFile` (`:145-197`), the 20-byte header and the payload,
//!   decoded with unityrs-core's codecs;
//! - `blob`: `TpkTypeTreeBlob` (`:215-240`) and the records it reads, the
//!   version list (`:463-469`), the classes and their entries
//!   (`blob::classes`, `:289-348`), the common strings and the string buffer
//!   (`blob::strings`, `:407-456`), and the node buffer (`blob::nodes`,
//!   `:351-397`);
//! - `tree`: `generate_node` (`:58-88`), a class's tree as its rows;
//! - `provider`: `get_typetree_node` (`:42-55`) and `get_item_for_version`
//!   (`:472-481`) with UnityPy's two caches, as [`TpkProvider`].
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
//!   one, each row's `m_Index` its position, `m_TypeFlags` left unset. Its
//!   `NODES_CACHE` keeps one tree per class record (`TpkUnityClass` hashes
//!   and compares by its name, base, flags and two roots, `:316-330`), so
//!   every entry of every class whose record is equal shares one tree. The
//!   walk reads only the release root, so the provider keeps one tree per
//!   release root: every entry UnityPy shares a tree among shares it here
//!   too, and so do entries of equal roots and unequal records, whose trees
//!   UnityPy builds twice, equal.
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
//! (`UnityVersion` is an `int`): a TPK's versions are its stored `u64`s, of
//! any type byte ([`UnityVersion::from_packed`]), and the request's is the
//! file's.
//!
//! **Loading** follows `TpkFile` and `GetDataBlob` (`Tpk.py:146-197`): the
//! 20-byte header (`<IbbbbIII`: the sizes are unsigned), the compressed
//! bytes, then the payload decoded as UnityPy decodes it, with unityrs-core's
//! codecs: LZ4 is `lz4.block.decompress(data, uncompressed_size)` with
//! liblz4 1.9.4's acceptance rules and the size a maximum
//! ([`unityrs_core::codec::Lz4`]), LZMA is `decompress_lzma`, a raw LZMA1
//! decoder run to the end of the stream with the header's size ignored
//! ([`unityrs_core::codec::Lzma`]), and brotli is refused
//! ([`TpkError::Unsupported`]: unityrs-core has no brotli decoder). The blob
//! is then read as `TpkTypeTreeBlob` reads it: each count as `Tpk.py` reads
//! it (a negative count read by `range()` is no records, one read as a struct
//! format, the version list's and the common-string indices', an error), a
//! class's roots each under its own flag (`:302-305`), every version kept as
//! its integer, a class listed twice keeping its last listing (a `dict`).
//!
//! **Limits, where UnityPy has none.** Loading is bounded before it
//! allocates ([`TpkError::TooLarge`]): the file at
//! [`TpkProvider::MAX_FILE_BYTES`], the decoded payload at
//! [`TpkProvider::MAX_DECODED_BYTES`], what the decoder holds (its output,
//! for LZMA allocated once at that bound, and an LZMA decoder's dictionary
//! and state, charged to a meter) at [`TpkProvider::MAX_DECODER_MEMORY`],
//! which a payload within the decoded bound never meets, whatever its header
//! declares, and each table of the blob at
//! [`TpkProvider::MAX_RECORDS`] records, refused when its count is read,
//! before any of its records. No count sizes an allocation: every table grows
//! as its records are read, each growth charged first. A string whose length
//! runs past the payload, which UnityPy decodes short, is an error, as is a
//! length prefix of more than 5 bytes. The walk stops with an error at a level
//! of [`Limits::max_tree_depth`] ([`unityrs_core::LimitKind::TreeDepth`]) or
//! past [`TpkProvider::MAX_TREE_ROWS`] rows, the rows still to walk counted
//! ([`unityrs_core::LimitKind::Count`]), so a node that lists itself or an
//! ancestor, on which UnityPy's walk never ends, is an error; a tree deeper
//! than the limit, which UnityPy builds, is one too. The walks of every
//! release root together build at most [`TpkProvider::MAX_TOTAL_ROWS`]
//! rows, kept or not: at load, one pass over the node buffer finds what
//! each root's walk would make without walking it (its tree's rows, or a
//! failure after at most its tree's rows, one past
//! [`TpkProvider::MAX_TREE_ROWS`] at most), and in the order of the roots'
//! ids a root whose walk would take the rows the walks before it build past
//! the bound is refused, before it is walked, with a
//! [`unityrs_core::LimitKind::Count`] limit naming the total it would reach
//! (a root whose walk fails counts the rows it may build before it fails).
//! So the provider keeps no more than that bound of rows, however many
//! classes an input asks for, and builds no more on any one thread, save
//! for walks the allocator refused ("Memory" below: such a walk is not kept,
//! and each later request for its root walks it again, each walk at most
//! one row past [`TpkProvider::MAX_TREE_ROWS`]); which roots it refuses
//! depends on the TPK alone, never on the order of the requests. Each is a [`ProviderError`] whose `limit` names the bound.
//! `PARITY.md` section 19.3 lists every one.
//!
//! **A compile reads no name.** Every tree of the TPK names its one
//! string table, and a tree's compile groups each class's children by name
//! (unityrs-core's `Tree::compile`, which reads each name buffer that
//! shares its length with another). The provider numbers its strings'
//! names once, at load, as the compile numbers a tree's
//! ([`NameTable`](unityrs_core::typetree::tree::NameTable), its work
//! counted and its tables charged on the provider's meter), and compiles
//! every tree with them (`Tree::compile_with_names`), so a compile reads
//! none of its rows' names (and at most the first 26 bytes of each type),
//! whatever their lengths and however many roots name them
//! (fix pass 13's sweep: each compile read them again, so a TPK whose many
//! release roots each name long strings of one length read them once a
//! root; `tests/names.rs`).
//!
//! **Threads.** The provider never waits (plan sections 3.16 and 4.1), so
//! threads that ask for one release root before its first answer is
//! published each walk it (and compile it, for a tree), and every one of
//! them then gets the first answer published; the others are dropped with
//! their charges. A thread walks a root at most once, unless the allocator
//! refused that walk: a refusal is not kept ([`unityrs_core::once`]'s
//! transient rule), so each later request for the root walks it again, on
//! any thread, each walk bounded at one row past
//! [`TpkProvider::MAX_TREE_ROWS`] (round 5). A compile the allocator refused
//! is not kept either, but the rows it compiled are: each later request
//! compiles them again and walks nothing (review round 6: these docs said
//! it walked the root again). So with T threads asking at
//! once and no walk refused, the walks build at most T times
//! [`TpkProvider::MAX_TOTAL_ROWS`] rows in all, and besides what is kept
//! the provider holds at most one unpublished tree per asking thread, each
//! at most one row past [`TpkProvider::MAX_TREE_ROWS`]
//! (`provider::tests::racing_first_requests_walk_a_root_at_most_once_per_thread`).
//!
//! **Memory.** The TPK is the provider's, not an input's: its tables, name
//! table, rows, trees and kept errors are charged to the provider's own
//! unlimited meter
//! (plan section 3.15's constant per context; the bounds above are what
//! bound it), as are the file's bytes while it loads and the copy of a
//! compressed payload its codec reads; the rows and tree of a release root
//! are kept once first published. Each is computed with no lock held and
//! published through [`unityrs_core::once::get_or_publish`]. A walk or
//! compile the allocator refuses is the exception: that depends on what else
//! the process held at that moment, not on the TPK or the request, so it is
//! returned unkept, `ProviderError::memory` set (the meter's refusals are
//! labelled `Reservation::Provider`), and the next request computes it
//! again: a refused walk is walked again, a refused compile compiled again
//! from the rows its walk kept. So is a load: liblzma's own allocations (an
//! LZMA payload's dictionary above all) that the allocator refuses are the
//! load's `OutOfMemory` ([`TpkError::Io`]), as a growth of the provider's is,
//! never a decode failure (round 6). A
//! kept error's text is shared (`ProviderError::detail` is an `Arc<str>`),
//! so the clone each later request gets allocates nothing.
//!
//! [`TypeTreeProvider`]: unityrs_core::provider::TypeTreeProvider
//! [`TypeTreeProvider::common_strings`]: unityrs_core::provider::TypeTreeProvider::common_strings
//! [`TypeTreeProvider::tree`]: unityrs_core::provider::TypeTreeProvider::tree
//! [`ProviderError`]: unityrs_core::provider::ProviderError
//! [`UnityVersion::from_packed`]: unityrs_core::UnityVersion::from_packed
//! [`Limits::max_tree_depth`]: unityrs_core::limits::Limits::max_tree_depth

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

mod blob;
mod file;
mod provider;
mod tree;

pub use provider::TpkProvider;

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16).
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Why a TPK did not load.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TpkError {
    /// The file could not be read, or the memory for what it holds could not
    /// be had (the allocator refused a growth, or liblzma's own allocation
    /// of an LZMA payload's dictionary: `OutOfMemory`).
    #[error("reading the TPK: {0}")]
    Io(#[from] std::io::Error),
    /// Its header or its blob is malformed, where UnityPy raises too: a bad
    /// magic, version, compression or data type, compressed bytes past the
    /// end, a count or a record past the end of the payload, invalid UTF-8;
    /// and, where UnityPy reads on (`PARITY.md` section 19.3), a string
    /// longer than the payload left or a length prefix of more than 5 bytes.
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

const _: () = {
    send_sync::<TpkError>();
    send_sync::<provider::TpkProvider>();
};

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
