// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPyBoost/TypeTreeHelper.cpp, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Reading and writing values through a typetree.
//!
//! `TypeTreeHelper.read_typetree` (`helpers/TypeTreeHelper.py:123-158`) picks
//! a reader by the object's size: with `byte_size > 0` UnityPyBoost reads the
//! object's bytes and nothing else ([`boost`]); with `byte_size == 0` the
//! pure-Python reader starts at `byte_start` in the whole file and may read on
//! past the object ([`python`]). Then `check_read` compares the bytes read
//! with `byte_size`, in either direction. UnityPyBoost has no writer, so
//! [`write`](mod@write) ports UnityPy's pure-Python `write_typetree` (`:161-179`,
//! `:407-458`), which does not mirror the reader (plan section 3.11).
//!
//! **Memory.** Every allocation a read makes for the value (a list, a map, a
//! tuple, a primitive run) is charged to [`ReadCtx::meter`], the input's,
//! before it is made, at [`crate::limits::heap_cost`] of its capacity, and
//! the charges ride in the [`Parsed`] (grow through
//! [`crate::limits::MeteredVec`], then hand its charge to the `Parsed`). A
//! refused charge is [`ReadDetail::Charge`] (kind `other`). The writer's
//! output grows the same way. What a read reserves before the bytes it holds
//! are read is bounded apart from the tree's width and the read's depth: a
//! dict at most [`DICT_FIRST`] entries, growing as its children are set;
//! the lists a read holds open at once, `max_prealloc` elements in all
//! (`Work::first_room`), each growing geometrically as it is filled (review
//! round 7 and its sweep: a dict reserved one entry a child, and each list
//! open `max_prealloc` of its own, so a `ReferencedObject` re-entering a
//! wide ref type, or vectors nested in vectors, held that many times over
//! for a few bytes of data; `tests/walk_reentry.rs`).
//!
//! **Work.** Every read and every write counts what it does toward its
//! open's work ([`crate::limits::Limits::max_decode_work`], the bound every
//! other kind of the open's work counts toward), whatever its outcome:
//! [`VALUE_WORK`] for each value it builds, writes or skips, and for each
//! element of a primitive run a read holds as a run, or each primitive a
//! write writes, its bytes (`Work`); each byte of a string a write copies,
//! or passes over to find its characters; [`STEP_WORK`] for each step that
//! builds no value: a slot a dict's table of positions grows by, an entry a
//! lookup by name passes, a name a writer's table indexes or probes (and
//! the bytes of a name it compares or hashes); and its ref-type lookups, a
//! ref type a lookup compares or indexes, or a probe, [`VALUE_WORK`] and the
//! name bytes it reads (`dict::RefIndex`). Objects may share their bytes,
//! and a read refused by a bound of its own (`max_zero_size_elements`,
//! `max_values_per_object`) keeps nothing, so K objects over the same bytes
//! repeated it K times with no bound on the input (fix pass 4's sweep: every
//! object of a small file over one vector of 2^22 empty elements, each
//! refused after 2^20); every object searches its file's one list of ref
//! types again, so K objects over R ref types took K x R steps (fix pass 5's
//! sweep); and a read failing at a class's first child had passed over all
//! of its children to size its dict (review round 6: K objects over a class
//! of `k` children, K x `k` steps; review round 7: the pass that stayed for
//! a class whose children repeat a name refused such reads with
//! `DecodeWork` where UnityPy read each to its own failure). No read or
//! write passes over a class's children before it reads or writes them.
//!
//! Each step is counted before it is taken, on the read's or the write's
//! `OpenWork`, which adds the steps to the open's meter a chunk
//! ([`WORK_CHUNK`]) at a time, in one atomic check-and-add: a chunk that
//! would take the open past its allowance is refused, and the read or the
//! write fails with [`LimitKind::DecodeWork`] (kind `other` for a read); one
//! whose own work, counted when it ends, took the open past its allowance
//! fails too (its own failure first, if it has one). So the reads and writes
//! that succeed never do more work together than the allowance, on any
//! number of threads, and each one in progress does at most a chunk and one
//! step past it before it stops (review round 6: a read added its work only
//! when it ended, so two reads on two threads both passed the start's check
//! and ran to their end past the allowance). Once the open's work has passed
//! its allowance, a read or a write is refused before it starts
//! (`tests/walk_work.rs`, `tests/walk_prepass.rs`).
//!
//! Owner: the scaffold for the types in this file (frozen); package B owns
//! [`read_typetree`]'s body and the three submodules.

pub mod boost;
mod dict;
pub mod python;
mod work;
pub mod write;

pub use dict::DICT_FIRST;
pub(crate) use dict::steps;
pub(crate) use work::{OpenWork, Work, limit_refused};
pub use work::{STEP_WORK, VALUE_WORK, WORK_CHUNK};

#[cfg(test)]
mod dict_tests;
#[cfg(test)]
mod linear_tests;
#[cfg(test)]
mod prepass_tests;

use std::sync::Arc;

use bytes::Bytes;

use crate::error::{LimitError, LimitKind};
use crate::files::SerializedType;
use crate::limits::{ChargeError, Limits, Meter, MeteredVec};
use crate::provider::{NoTree, ProviderError};
use crate::raw::RawStr;
use crate::stream::{Endian, ReaderKind, StreamRefusal};
use crate::typetree::{NodeId, Tree, TreeError};
use crate::value::{Built, Parsed, Value};

/// How a float32 signalling NaN reaches the value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FloatMode {
    /// As UnityPyBoost delivers it: `PyFloat_FromDouble` on x86-64 quiets a
    /// signalling NaN (`7f800001` becomes `7fc00001`; `CANONICAL_DUMP.md`
    /// section 7). The oracle's behaviour, and the default.
    #[default]
    UnityPy,
    /// The file's bits; `save_typetree(parse())` then round-trips them.
    Lossless,
}

/// The writer's alignment rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WriteRules {
    /// UnityPy's `write_value`: an array element whose node carries the align
    /// flag aligns after *every* element, where the reader aligns once after a
    /// run. The oracle's L3 behaviour, and the default.
    #[default]
    UnityPy,
    /// Write as the reader that built the value reads ([`Parsed::read_by`]).
    /// For UnityPyBoost's: align once after a flagged primitive or `pair`
    /// run, write no data where the reader skipped it (a `class` whose size
    /// field is 0, or a matched ref type with no tree), and take a type name
    /// to its first NUL. For the pure-Python reader's: take a type name
    /// whole, write a flagged array's elements as `read_value_array` reads
    /// them (each kind without its own trailing alignment, the run aligned
    /// once after; a `pair`'s primitive, string or bytes children without
    /// their own flags, its 16-bit ones unswapped; a class's registry
    /// children with no registry check; a run of 16-bit integers with their
    /// signedness swapped), align no empty string after itself (so not at
    /// all where that reader reads no flag: a run's element, a flagged
    /// `pair` run's child), and write no data for an empty `class` or a
    /// matched ref type with no tree. A value either reader reads through a tree that
    /// repeats no name writes back to bytes the same reader reads as the
    /// same value (padding as zeros), unless the pure-Python reader moved
    /// its position other than forward over what it holds (a negative
    /// `TypelessData` length, or one past the end) or started at a position
    /// not a multiple of 4 (it aligns to the file, the writer to the
    /// object); a repeated name keeps only its last value, so no writer
    /// gives back the others' bytes (plan 3.11; a `PARITY.md` Tier C
    /// option; `tests/walk_mirror.rs`).
    MirrorReader,
}

/// The walker's options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// Float32 signalling NaNs.
    pub floats: FloatMode,
    /// Depth, value-count and zero-size bounds.
    pub limits: Limits,
}

/// `assetsfile.ref_types` as the reader sees it (UnityPyBoost
/// `TypeTreeHelper.cpp:585-681`; the pure-Python writer, `TypeTreeHelper.py:88-109`).
#[derive(Clone, Copy, Debug)]
pub enum RefTypes<'a> {
    /// No assets file at all (UnityPyBoost: `ref_type` error on use).
    NoFile,
    /// The file has no `ref_types` attribute (format below 20).
    Absent,
    /// The file's ref types, in file order (may be empty).
    List(&'a [SerializedType]),
}

/// What the reader needs besides the bytes and the tree.
#[derive(Clone, Copy, Debug)]
pub struct ReadCtx<'a> {
    /// The file's byte order.
    pub endian: Endian,
    /// For `ReferencedObject` data.
    pub ref_types: RefTypes<'a>,
    /// Floats and limits.
    pub options: &'a ReadOptions,
    /// The input's meter: every allocation of the value is charged here
    /// first, and the [`Parsed`] keeps the charges (the object's file's
    /// [`crate::files::SerializedFile::meter`]).
    pub meter: &'a Arc<Meter>,
}

/// What the writer needs besides the value and the tree.
#[derive(Clone, Copy, Debug)]
pub struct WriteCtx<'a> {
    /// The file's byte order.
    pub endian: Endian,
    /// For `ReferencedObject` data.
    pub ref_types: RefTypes<'a>,
    /// Alignment rules.
    pub rules: WriteRules,
    /// The depth and values bounds (`max_value_depth`,
    /// `max_values_per_object`).
    pub limits: &'a Limits,
}

/// The canonical dump's L2 error kinds (`CANONICAL_DUMP.md` section 4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum L2Kind {
    /// No tree: none embedded, none from a provider.
    NoTypetree,
    /// The read ended `bytes_read` bytes in, and that is not `byte_size`.
    CheckRead,
    /// A read (not an alignment) past the object's end, on the Boost path.
    OutOfBounds,
    /// A negative length.
    NegativeLength,
    /// A `ReferencedObject` whose type is not in `ref_types`, or no `ref_types`.
    RefType,
    /// The value does not fit its tree (an encoder bug).
    Encode,
    /// Anything else, our limits included.
    Other,
}

impl L2Kind {
    /// The dump's string for the kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoTypetree => "no_typetree",
            Self::CheckRead => "check_read",
            Self::OutOfBounds => "out_of_bounds",
            Self::NegativeLength => "negative_length",
            Self::RefType => "ref_type",
            Self::Encode => "encode",
            Self::Other => "other",
        }
    }
}

/// A reader failure, with its kind decided structurally (never parsed from a message).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?} at byte {at}: {detail}")]
pub struct ReadError {
    /// The dump's kind.
    pub kind: ReadErrorKind,
    /// Offset from the start of the read.
    pub at: u64,
    /// What happened.
    pub detail: ReadDetail,
}

/// A reader failure's L2 kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadErrorKind {
    /// `out_of_bounds`.
    OutOfBounds,
    /// `negative_length`.
    NegativeLength,
    /// `ref_type`.
    RefType,
    /// `other`.
    Other,
}

/// What a reader failure was.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReadDetail {
    /// A read needed bytes past the end.
    #[error("needs {need} bytes past the end")]
    Eof {
        /// Bytes needed.
        need: u64,
    },
    /// A negative length.
    #[error("negative length {0}")]
    NegativeLength(i64),
    /// An `Array` node without two children ("Array node must have 2 children").
    #[error("Array node must have 2 children")]
    ArrayChildren,
    /// A `pair` node without two children ("Pair node must have 2 children").
    #[error("Pair node must have 2 children")]
    PairChildren,
    /// `ReferencedObject` data with no `ref_types` to resolve it.
    #[error("no SerializedFile.ref_types")]
    NoRefTypes,
    /// `ReferencedObject` data whose `(class, ns, asm)` is not in `ref_types`.
    #[error("Failed to get ref type node")]
    RefTypeNotFound,
    /// The pure-Python reader's array path only: an aligned `ReferencedObject`
    /// element whose class is empty or whose matched ref type has no tree
    /// (type trees disabled) calls `read_value(None, ...)`, an `AttributeError`
    /// (kind `other`). UnityPyBoost and the pure-Python scalar path skip
    /// `data` instead, as for an empty class (plan R14).
    #[error("'NoneType' object has no attribute 'm_MetaFlag'")]
    RefTypeNoTree,
    /// A pure-Python-path `TypeError` or `struct.error` (aligned `SInt8` or `TypelessData` arrays).
    #[error("python reader: {0}")]
    Python(&'static str),
    /// The stream reader's refusal (a file loaded from a path): a negative
    /// or huge `byte_start`, or a `TypelessData` length below -1 on the
    /// pure-Python path (kind `other`).
    #[error("{}: {}", .0.exception(), .0)]
    Stream(StreamRefusal),
    /// A bound from [`Limits`] on work or depth.
    #[error(transparent)]
    Limit(LimitError),
    /// The input's meter or the budget refused the value's memory.
    #[error(transparent)]
    Charge(ChargeError),
    /// A ref type's tree did not compile.
    #[error(transparent)]
    Tree(TreeError),
}

/// Why reading an object failed: the L2 outcome other than success.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// No tree for the class: UnityPy raises the exception `why` names
    /// (its TPK's; review round 11: nothing told them apart).
    #[error("no typetree for class {class_id}")]
    NoTypeTree {
        /// The object's class.
        class_id: i32,
        /// Which exception: `None` where no provider says (with no provider
        /// at all; UnityPy always has its TPK).
        why: Option<NoTree>,
    },
    /// The reader failed; no value.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The read finished but did not end at `byte_size`. The value rides
    /// along: it is what `parse_as_dict(check_read=False)` returns. Shared,
    /// not copied, when the error is cloned (a published result is handed to
    /// every caller), and still charged to its input while any copy lives.
    #[error("Expected to read {expected} bytes, but only read {} bytes", .parsed.bytes_read)]
    CheckRead {
        /// `byte_size`.
        expected: u32,
        /// The full value and where the read ended.
        parsed: Arc<Parsed>,
    },
    /// The object's tree did not compile.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// A provider failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The `MonoBehaviour` script hook's dereference of `m_Script` failed
    /// with anything but a null pointer, which UnityPy raises
    /// (`files/ObjectReader.py:332-350`; [`crate::env::Environment::object_tree`]).
    #[error("m_Script: {0}")]
    Deref(Arc<crate::env::DerefError>),
}

impl crate::once::CellError for ParseError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Read(r) => match &r.detail {
                ReadDetail::Charge(c) => c.is_transient(),
                ReadDetail::Tree(t) => t.is_transient(),
                _ => false,
            },
            Self::Tree(t) => t.is_transient(),
            // A provider is caller code: its failure is not the input's.
            Self::Provider(_) => true,
            Self::Deref(d) => d.is_transient(),
            Self::NoTypeTree { .. } | Self::CheckRead { .. } => false,
        }
    }

    fn is_meter_refusal(&self) -> bool {
        match self {
            Self::Read(r) => match &r.detail {
                ReadDetail::Charge(c) => c.is_meter_refusal(),
                ReadDetail::Tree(t) => t.is_meter_refusal(),
                _ => false,
            },
            Self::Tree(t) => t.is_meter_refusal(),
            Self::Deref(d) => d.is_meter_refusal(),
            Self::Provider(_) | Self::NoTypeTree { .. } | Self::CheckRead { .. } => false,
        }
    }

    fn heap_bytes(&self) -> u64 {
        match self {
            // The value's own heap rides in its `Parsed`'s charge.
            Self::CheckRead { .. } => crate::error::heap_of(std::mem::size_of::<Parsed>() + 16),
            Self::Provider(p) => crate::error::heap_of(p.detail.len()),
            Self::Deref(d) => crate::error::arc_heap_of(size_of::<crate::env::DerefError>())
                .saturating_add(d.heap_bytes()),
            _ => 0,
        }
    }

    /// A [`Limits`] bound, so kind `other` (R16).
    fn work_refused(limit: LimitError) -> Self {
        Self::Read(ReadError {
            kind: ReadErrorKind::Other,
            at: 0,
            detail: ReadDetail::Limit(limit),
        })
    }
}

impl ParseError {
    /// The canonical dump's kind for this outcome. A read that ended before
    /// its start (a negative `bytes_read`, the pure-Python reader's after a
    /// negative `TypelessData` length) is `other`: the oracle reads the count
    /// from the message with `(\d+)`, which takes no sign
    /// (`tools/parity/dump_records.py` `error_kind`; plan 3.10).
    #[must_use]
    pub fn l2_kind(&self) -> L2Kind {
        match self {
            Self::NoTypeTree { .. } => L2Kind::NoTypetree,
            Self::CheckRead { parsed, .. } if parsed.bytes_read < 0 => L2Kind::Other,
            Self::CheckRead { .. } => L2Kind::CheckRead,
            Self::Read(e) => match e.kind {
                ReadErrorKind::OutOfBounds => L2Kind::OutOfBounds,
                ReadErrorKind::NegativeLength => L2Kind::NegativeLength,
                ReadErrorKind::RefType => L2Kind::RefType,
                ReadErrorKind::Other => L2Kind::Other,
            },
            Self::Tree(_) | Self::Provider(_) | Self::Deref(_) => L2Kind::Other,
        }
    }
}

/// Why writing a value failed (the dump's L3 `"error"`). Each variant
/// stands for the exceptions [`Self::exceptions`] names, the classes UnityPy
/// 1.25.2's `write_typetree` raises there; a variant UnityPy has no
/// counterpart for names none.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// A child name missing from the dict, or a `ReferencedObject`'s `type`
    /// or one of its names (`KeyError`).
    #[error("missing key {0:?}")]
    MissingKey(RawStr),
    /// A value of the wrong type for its node. `expected` says what the node
    /// takes, and which exception UnityPy raises: `struct.error` for `int`
    /// and `float`; `AttributeError` for a `str` (`value.encode`), an
    /// attribute, a ref type with a tree (`None.m_MetaFlag`) and a `type`
    /// that is not a dict (`getattr(typ, "class")`); `TypeError` for
    /// `bytes`, a sized iterable (`len`) and a `ReferencedObject`'s dict
    /// (`value[...]`); `TypeError`, `KeyError` or `IndexError` for a
    /// sequence of two (`value[0]` of an `int`, a `dict` or a short
    /// sequence). A modelled attribute is the writer's own refusal of a
    /// dunder name, which UnityPy reads and may write (`PARITY.md` section
    /// 19.3).
    #[error("{node:?}: expected {expected}")]
    TypeMismatch {
        /// The node's name.
        node: RawStr,
        /// What the node takes.
        expected: &'static str,
    },
    /// An integer outside its node's width (`struct.error`), or a finite
    /// float that overflows `f32` (`OverflowError`).
    #[error("{node:?}: value out of range")]
    OutOfRange {
        /// The node's name.
        node: RawStr,
    },
    /// `ReferencedObject` data with no assets file, or an empty `ref_types`
    /// list (the writer raises where the reader does not; `TypeTreeHelper.py:99-100`):
    /// `ValueError`.
    #[error("SerializedFile has no ref_types")]
    NoRefTypes,
    /// `ReferencedObject` data against a file with no `ref_types` attribute
    /// ([`RefTypes::Absent`]: a `SerializedFile` below format 20, which
    /// UnityPy never sets it on, `files/SerializedFile.py:305-308`): reading
    /// `assetfile.ref_types` raises `AttributeError` (`TypeTreeHelper.py:99`).
    #[error("'SerializedFile' object has no attribute 'ref_types'")]
    NoRefTypesAttribute,
    /// `ReferencedObject` data whose type is not in `ref_types`
    /// (`ValueError`).
    #[error("Referenced type not found")]
    RefTypeNotFound,
    /// A malformed node the writer reached: a `pair`'s or an `Array`'s
    /// missing child (`IndexError`), or a matched ref type's rows that make
    /// no tree, which UnityPy refuses when it parses the file (an
    /// `IndexError` too, [`TreeError`]).
    #[error("malformed node {0:?}")]
    Malformed(RawStr),
    /// The object has no tree to write with: UnityPy raises the same
    /// exception for a write as for a read (`files/ObjectReader.py:264-307`;
    /// the read side's [`ParseError::NoTypeTree`]).
    #[error("no typetree for class {class_id}")]
    NoTypeTree {
        /// The object's class.
        class_id: i32,
        /// Which exception, as the read side's.
        why: Option<NoTree>,
    },
    /// Resolving the object's tree failed (a tree that does not compile, a
    /// provider's failure): the writer never ran.
    #[error("typetree: {0}")]
    Tree(Box<ParseError>),
    /// A bound from [`Limits`].
    #[error(transparent)]
    Limit(LimitError),
    /// The input's meter or the budget refused the output's memory.
    #[error(transparent)]
    Charge(ChargeError),
}

impl WriteError {
    /// The classes, by `__name__` (`error` is `struct.error`), of the
    /// exceptions UnityPy 1.25.2's `write_typetree` (`TypeTreeHelper.py:407-458`
    /// and `streams/EndianBinaryWriter.py`) raises where the writer reports
    /// this error, as the variants' docs give them; `tests/walk_fixtures.rs`
    /// checks against this every raise in the cases, the value cases and
    /// W2's matrix `tools/fixtures/walk.py` records.
    /// None for an error UnityPy has no counterpart for: a bound of
    /// [`Limits`] or a refused charge (`PARITY.md` section 19.3), a dunder
    /// attribute ([`Self::TypeMismatch`]), and a tree that failed before the
    /// writer ran ([`Self::Tree`]: the read side's error); a
    /// [`Self::NoTypeTree`] with no `why` says nothing of UnityPy's.
    #[must_use]
    pub fn exceptions(&self) -> &'static [&'static str] {
        match self {
            Self::MissingKey(_) => &["KeyError"],
            Self::TypeMismatch { expected, .. } => match *expected {
                "int" | "float" => &["error"],
                "str"
                | "an object with this attribute"
                | "a ref type with a tree"
                | "a dict or an object with class, ns and asm" => &["AttributeError"],
                "bytes" | "a sized iterable" | "dict" => &["TypeError"],
                "a sequence of two" => &["TypeError", "KeyError", "IndexError"],
                _ => &[],
            },
            Self::OutOfRange { .. } => &["error", "OverflowError"],
            Self::NoRefTypes | Self::RefTypeNotFound => &["ValueError"],
            Self::NoRefTypesAttribute => &["AttributeError"],
            Self::Malformed(_) => &["IndexError"],
            Self::NoTypeTree { why, .. } => match why {
                Some(NoTree::UnknownClass) => &["KeyError"],
                Some(NoTree::NoEntry) => &["ValueError"],
                Some(NoTree::NoReleaseRoot) => &["AssertionError"],
                Some(NoTree::NoVersion) => &["NotImplementedError"],
                None => &[],
            },
            Self::Tree(_) | Self::Limit(_) | Self::Charge(_) => &[],
        }
    }
}

/// `TypeTreeHelper.read_typetree` over bytes (UnityPy's memory reader):
/// [`boost::read`] over the Python slice `file[byte_start : byte_start +
/// byte_size]` ([`crate::stream::py_read`]: short at the end of a truncated
/// file, from the end for a negative start) when `byte_size > 0`, else
/// [`python::read`] from `byte_start`; then, with `check_read`, `bytes_read
/// != byte_size` is [`ParseError::CheckRead`] carrying the value.
pub fn read_typetree(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
) -> Result<Parsed, ParseError> {
    read_typetree_with_reader(
        file,
        byte_start,
        byte_size,
        tree,
        ctx,
        check_read,
        ReaderKind::Memory,
    )
}

/// [`read_typetree`] from the file's reader of `kind`
/// ([`crate::files::SerializedFile::reader_kind`]): `reset()` sets the
/// position to `byte_start`, which the stream reader refuses when negative
/// or at or above 2^63 ([`ReadDetail::Stream`], kind `other`), as UnityPy's
/// does on every machine; a `byte_start` past the largest file the file
/// system takes is UnityPy's `OSError` on that machine, and taken here
/// ([`crate::stream::ReaderKind`], `PARITY.md` section 19.3). The Boost
/// path is given `read_bytes(byte_size)` from there (on both readers the
/// bytes that exist), and the pure-Python reader reads by the reader's
/// rules ([`python::read_with_reader`]).
pub fn read_typetree_with_reader(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
    kind: ReaderKind,
) -> Result<Parsed, ParseError> {
    // UnityPy files/ObjectReader.py:214-215, :251: `reset()`.
    let start = kind.seek(byte_start).map_err(|e| stream_refused(0, e))?;
    // UnityPy TypeTreeHelper.py:146-153: `byte_size and read_typetree_boost`.
    let parsed = if byte_size > 0 {
        // UnityPy TypeTreeHelper.py:147: `reader.read_bytes(byte_size)`: on
        // the memory reader a Python slice of the file from `byte_start`.
        let (range, _) = kind
            .read(file.len(), start, i64::from(byte_size))
            .map_err(|e| stream_refused(0, e))?;
        boost::read(&file.slice(range), tree, ctx)?
    } else {
        python::read_with_reader(file, start, tree, ctx, kind)?
    };
    // UnityPy TypeTreeHelper.py:155-156.
    if check_read && parsed.bytes_read != i64::from(byte_size) {
        return Err(ParseError::CheckRead {
            expected: byte_size,
            parsed: Arc::new(parsed),
        });
    }
    Ok(parsed)
}

/// The stream reader's refusal at `at` bytes into a read (kind `other`, as
/// the oracle maps a `ValueError` or `OverflowError` it names no kind for).
pub(crate) fn stream_refused(at: u64, e: StreamRefusal) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Other,
        at,
        detail: ReadDetail::Stream(e),
    }
}

/// A value as a reader returns it to its parent: one that owns no heap
/// (a scalar, a string or bytes sliced from the file), pushed as it is, or
/// a [`Built`] list, map, tuple or run that carries its charge (plan 3.9).
/// A plain scalar costs no charge record, so the hot path stays one move.
#[derive(Debug)]
pub(crate) enum Out {
    /// Owns no heap.
    Plain(Value),
    /// Owns heap, with its charge.
    Built(Built),
}

impl Out {
    /// Appends to a list's or a tuple's items, the charge going along.
    pub(crate) fn push_to(self, items: &mut MeteredVec<Value>) -> Result<(), ChargeError> {
        match self {
            Self::Plain(v) => items.push(v),
            Self::Built(b) => items.push_built(b),
        }
    }

    /// Appends a dict entry, the charge going along.
    pub(crate) fn push_entry(
        self,
        entries: &mut MeteredVec<(NodeId, Value)>,
        key: NodeId,
    ) -> Result<(), ChargeError> {
        match self {
            Self::Plain(v) => entries.push((key, v)),
            Self::Built(b) => entries.push_entry(key, b),
        }
    }

    /// Replaces entry `pos`'s value (`PyDict_SetItem` on a key already in
    /// the dict: first position, last value).
    pub(crate) fn set_entry(self, entries: &mut MeteredVec<(NodeId, Value)>, pos: usize) {
        match self {
            Self::Plain(v) => {
                if let Some(e) = entries.as_mut_slice().get_mut(pos) {
                    e.1 = v;
                }
            }
            Self::Built(b) => {
                entries.set_entry(pos, b);
            }
        }
    }

    /// The `Built` a [`crate::value::ValueBuilder::finish`] takes. A plain
    /// value owns no heap, so `scalar` always takes it; were one to own
    /// heap, the read fails rather than keep it uncharged.
    pub(crate) fn into_built(
        self,
        builder: &crate::value::ValueBuilder,
        at: u64,
    ) -> Result<Built, ReadError> {
        match self {
            Self::Built(b) => Ok(b),
            Self::Plain(v) => builder.scalar(v).map_err(|_| ReadError {
                kind: ReadErrorKind::Other,
                at,
                detail: ReadDetail::Python("a plain value owned heap"),
            }),
        }
    }
}

/// UnityPyBoost's float widening: `PyFloat_FromDouble((double)f)`
/// (UnityPy TypeTreeHelper.cpp:172-175) on x86-64 sets a signalling NaN's
/// quiet bit; [`FloatMode::Lossless`] keeps the file's bits.
pub(crate) fn f32_bits(bits: u32, mode: FloatMode) -> u32 {
    let nan = bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0;
    if nan && mode == FloatMode::UnityPy {
        bits | 0x0040_0000
    } else {
        bits
    }
}

/// An `i64` as a Python `int`'s value variant.
pub(crate) fn int(v: i64) -> Value {
    Value::Int(v)
}

/// A `u64` as a Python `int`'s value variant: `UInt` only above `i64::MAX`.
pub(crate) fn uint(v: u64) -> Value {
    i64::try_from(v).map_or(Value::UInt(v), Value::Int)
}

/// `CPython` 3.11's `PyUnicode_GET_LENGTH(cls) == 0` on whatever object the
/// reader made for a `ReferencedObject`'s `class` (UnityPy
/// TypeTreeHelper.cpp:637): the macro reads the third machine word of the
/// object, which is a `str`'s length, an `int`'s or a `bool`'s `ob_size`
/// (0 only for zero and `False`), a `float`'s value bits, a `bytes`',
/// `list`'s or `tuple`'s `ob_size` and a `dict`'s `ma_used`; so it is 0
/// exactly for an empty string, `0`, `False`, `+0.0`, and an empty bytes,
/// list, tuple or dict (plan R14).
pub(crate) fn py_size_field_is_zero(v: &Value) -> bool {
    match v {
        Value::Str(s) => s.is_empty(),
        Value::Int(i) => *i == 0,
        Value::UInt(_) => false,
        Value::Bool(b) => !*b,
        Value::F32(b) => f64::from(f32::from_bits(*b)).to_bits() == 0,
        Value::F64(b) => *b == 0,
        Value::Bytes(b) => b.is_empty(),
        Value::Prims(p) => p.is_empty(),
        Value::List(l) => l.is_empty(),
        Value::Tuple(t) => t.is_empty(),
        Value::Dict(m) => m.is_empty(),
    }
}

/// A [`Limits`] bound crossed while reading: kind `other` (R16).
pub(crate) fn limit_error(at: u64, kind: LimitKind, value: u64, max: u64) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Other,
        at,
        detail: ReadDetail::Limit(LimitError { kind, value, max }),
    }
}

/// A refused charge while reading: kind `other` (plan 3.10, "Bounds").
pub(crate) fn charge_error(at: u64, e: ChargeError) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Other,
        at,
        detail: ReadDetail::Charge(e),
    }
}

/// The capacity a list of `len` elements may reserve before its elements
/// are read: `min(len, remaining / min_size, room)`, `room` what the read's
/// open lists leave of `max_prealloc` (plan 3.10, "Bounds";
/// `Work::first_room`); it then grows by [`grow_step`].
pub(crate) fn first_reserve(len: usize, remaining: usize, min_size: u32, room: usize) -> usize {
    let backed = if min_size == 0 {
        usize::MAX
    } else {
        remaining / (min_size as usize)
    };
    len.min(backed).min(room)
}

/// Before pushing element `items.len()` of `len`: grows a full vector
/// geometrically, never past `len`, so the list holds no spare capacity once
/// it is complete, and, past its first reservation, no more than twice what
/// it has filled; growths that move it copy each element a bounded number
/// of times (review round 7's sweep: growing by exact steps of
/// `max_prealloc`, a list of `n` elements made `n` / `max_prealloc` moves,
/// some `n`² / `max_prealloc` copies in all, unless the allocator grew it
/// in place).
pub(crate) fn grow_step<T>(items: &mut MeteredVec<T>, len: usize) -> Result<(), ChargeError> {
    if items.len() < items.capacity() {
        return Ok(());
    }
    items.reserve_within(1, len)
}
