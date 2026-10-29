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
//! output grows the same way.
//!
//! Owner: the scaffold for the types in this file (frozen); package B owns
//! [`read_typetree`]'s body and the three submodules.

// STUB(B): delete this allow when `read_typetree` is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    unused_variables
)]

pub mod boost;
pub mod python;
pub mod write;

use std::sync::Arc;

use bytes::Bytes;

use crate::error::LimitError;
use crate::files::SerializedType;
use crate::limits::{ChargeError, Limits, Meter};
use crate::provider::ProviderError;
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{Tree, TreeError};
use crate::value::Parsed;

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
    /// Align as the reader reads: once after a flagged primitive or pair run.
    /// Round-trips every value UnityPy's writer pads (a `PARITY.md` Tier C option).
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
/// `TypeTreeHelper.cpp:585-682`; the pure-Python writer, `TypeTreeHelper.py:88-109`).
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
    /// Depth bound.
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
    /// No tree for the class (`TypeTreeError` in UnityPy).
    #[error("no typetree for class {class_id}")]
    NoTypeTree {
        /// The object's class.
        class_id: i32,
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
            Self::NoTypeTree { .. } | Self::CheckRead { .. } => false,
        }
    }

    fn heap_bytes(&self) -> u64 {
        match self {
            // The value's own heap rides in its `Parsed`'s charge.
            Self::CheckRead { .. } => crate::error::heap_of(std::mem::size_of::<Parsed>() + 16),
            Self::Provider(p) => crate::error::heap_of(p.detail.len()),
            _ => 0,
        }
    }
}

impl ParseError {
    /// The canonical dump's kind for this outcome.
    #[must_use]
    pub fn l2_kind(&self) -> L2Kind {
        match self {
            Self::NoTypeTree { .. } => L2Kind::NoTypetree,
            Self::CheckRead { .. } => L2Kind::CheckRead,
            Self::Read(e) => match e.kind {
                ReadErrorKind::OutOfBounds => L2Kind::OutOfBounds,
                ReadErrorKind::NegativeLength => L2Kind::NegativeLength,
                ReadErrorKind::RefType => L2Kind::RefType,
                ReadErrorKind::Other => L2Kind::Other,
            },
            Self::Tree(_) | Self::Provider(_) => L2Kind::Other,
        }
    }
}

/// Why writing a value failed (the dump's L3 `"error"`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// A child name missing from the dict (Python `KeyError`).
    #[error("missing key {0:?}")]
    MissingKey(RawStr),
    /// A value of the wrong type for its node (`struct.error`, `TypeError`).
    #[error("{node:?}: expected {expected}")]
    TypeMismatch {
        /// The node's name.
        node: RawStr,
        /// What the node takes.
        expected: &'static str,
    },
    /// An integer outside its node's width, or a finite float that overflows `f32`.
    #[error("{node:?}: value out of range")]
    OutOfRange {
        /// The node's name.
        node: RawStr,
    },
    /// `ReferencedObject` data with no `ref_types`, or an empty list
    /// (the writer raises where the reader does not; `TypeTreeHelper.py:99-100`).
    #[error("SerializedFile has no ref_types")]
    NoRefTypes,
    /// `ReferencedObject` data whose type is not in `ref_types`.
    #[error("Referenced type not found")]
    RefTypeNotFound,
    /// A malformed node the writer reached (`pair` or `Array` children).
    #[error("malformed node {0:?}")]
    Malformed(RawStr),
    /// A bound from [`Limits`].
    #[error(transparent)]
    Limit(LimitError),
    /// The input's meter or the budget refused the output's memory.
    #[error(transparent)]
    Charge(ChargeError),
}

/// `TypeTreeHelper.read_typetree`: [`boost::read`] over the Python slice
/// `file[byte_start : byte_start + byte_size]` ([`crate::stream::py_read`]:
/// short at the end of a truncated file, from the end for a negative start)
/// when `byte_size > 0`, else [`python::read`] from `byte_start`; then, with
/// `check_read`, `bytes_read != byte_size` is [`ParseError::CheckRead`]
/// carrying the value.
pub fn read_typetree(
    file: &Bytes,
    byte_start: i128,
    byte_size: u32,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    check_read: bool,
) -> Result<Parsed, ParseError> {
    todo!()
}
