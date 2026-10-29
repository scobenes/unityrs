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

pub mod boost;
mod dict;
pub mod python;
pub mod write;

use std::sync::Arc;

use bytes::Bytes;

use crate::error::{LimitError, LimitKind};
use crate::files::SerializedType;
use crate::limits::{ChargeError, Limits, Meter, MeteredVec};
use crate::provider::ProviderError;
use crate::raw::RawStr;
use crate::stream::{Endian, py_read};
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
    /// The `MonoBehaviour` script hook's dereference of `m_Script` failed
    /// with anything but a null pointer, which UnityPy raises
    /// (`files/ObjectReader.py:332-350`; [`crate::env::Environment::object_tree`]).
    #[error("m_Script: {0}")]
    Deref(Box<crate::env::DerefError>),
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
            Self::Deref(d) => crate::error::heap_of(std::mem::size_of::<crate::env::DerefError>())
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
    /// (`tools/parity/unitypy_dump.py` `error_kind`; plan 3.10).
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
    /// The object has no tree to write with: UnityPy raises the same
    /// `TypeTreeError` for a write as for a read (`files/ObjectReader.py:296-307`;
    /// the read side's [`ParseError::NoTypeTree`]).
    #[error("no typetree for class {class_id}")]
    NoTypeTree {
        /// The object's class.
        class_id: i32,
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
    // UnityPy TypeTreeHelper.py:146-153: `byte_size and read_typetree_boost`.
    let parsed = if byte_size > 0 {
        // UnityPy TypeTreeHelper.py:147: `reader.read_bytes(byte_size)`, a
        // Python slice of the file from `byte_start`.
        let (range, _) = py_read(file.len(), byte_start, i64::from(byte_size));
        boost::read(&file.slice(range), tree, ctx)?
    } else {
        python::read(file, byte_start, tree, ctx)?
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

/// The capacity a list of `len` elements reserves before its elements are
/// read: `min(len, remaining / min_size, max_prealloc)` (plan 3.10,
/// "Bounds"); it then grows by [`grow_step`].
pub(crate) fn first_reserve(
    len: usize,
    remaining: usize,
    min_size: u32,
    max_prealloc: usize,
) -> usize {
    let backed = if min_size == 0 {
        usize::MAX
    } else {
        remaining / (min_size as usize)
    };
    len.min(backed).min(max_prealloc)
}

/// Before pushing element `items.len()` of `len`: grows a full vector by
/// at most `max_prealloc`, exactly, never past `len`, so the list holds no
/// spare capacity once it is complete.
pub(crate) fn grow_step<T>(
    items: &mut MeteredVec<T>,
    len: usize,
    max_prealloc: usize,
) -> Result<(), ChargeError> {
    if items.len() < items.capacity() {
        return Ok(());
    }
    let step = len
        .saturating_sub(items.len())
        .min(max_prealloc.max(1))
        .max(1);
    items.reserve_exact(step)
}

/// The reader's limits on work, shared by both readers (plan 3.10, "Bounds").
#[derive(Debug)]
pub(crate) struct Work {
    pub(crate) values: u64,
    pub(crate) zero_size: u64,
    pub(crate) depth: u32,
    pub(crate) limits: Limits,
}

impl Work {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            values: 0,
            zero_size: 0,
            depth: 0,
            limits,
        }
    }

    /// Counts `n` values against `max_values_per_object`.
    pub(crate) fn values(&mut self, n: u64, at: u64) -> Result<(), ReadError> {
        self.values = self.values.saturating_add(n);
        if self.values > self.limits.max_values_per_object {
            return Err(limit_error(
                at,
                LimitKind::ValuesPerObject,
                self.values,
                self.limits.max_values_per_object,
            ));
        }
        Ok(())
    }

    /// Counts one element whose minimum size is 0.
    pub(crate) fn zero_size(&mut self, at: u64) -> Result<(), ReadError> {
        self.zero_size = self.zero_size.saturating_add(1);
        if self.zero_size > self.limits.max_zero_size_elements {
            return Err(limit_error(
                at,
                LimitKind::ZeroSizeElements,
                self.zero_size,
                self.limits.max_zero_size_elements,
            ));
        }
        Ok(())
    }

    /// Enters one level of value nesting.
    pub(crate) fn enter(&mut self, at: u64) -> Result<(), ReadError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.limits.max_value_depth {
            return Err(limit_error(
                at,
                LimitKind::ValueDepth,
                u64::from(self.depth),
                u64::from(self.limits.max_value_depth),
            ));
        }
        Ok(())
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}
