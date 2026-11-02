// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPy/streams/EndianBinaryReader.py. See THIRD_PARTY_LICENSES.
//! UnityPy's pure-Python reader (`helpers/TypeTreeHelper.py:182-361`), for
//! objects with `byte_size == 0`. It starts at `byte_start` in the whole
//! `SerializedFile` and may read on past the object, with Python's reader
//! rules at any position ([`crate::stream`]'s helpers; the position may go
//! negative). **Alignment is relative to the file**, not the object:
//! `align_stream` rounds the absolute position. Its differences from
//! UnityPyBoost are `docs/CANONICAL_DUMP.md` section 7's list: aligned
//! primitive arrays become tuples, with 16-bit signedness swapped; an
//! aligned `SInt8` array raises at every length (`read_byte_array(reader,
//! size)` before any loop), and an array of aligned `TypelessData` raises
//! only when its length is above 0 (`read_bytes()` inside the loop; an empty
//! one reads as `[]`); a negative or out-of-range string
//! length reads as `""`, and a string of length 0 or read as `""` that way
//! is not aligned after (`streams/EndianBinaryReader.py:266-273` returns
//! before `align_stream`); in an array whose aligned element is a vector, a
//! negative inner length reads as `[]`, unless the inner element is an
//! aligned primitive, whose `struct` format rejects it (`struct.error`,
//! `other`); arrays of aligned classes align once and skip the
//! registry check; `TypelessData` length -1 reads to the end of the file and
//! other negative lengths follow Python slice semantics (so `bytes_read` can
//! be negative), on the memory reader: a file loaded from a path is read by
//! the stream reader's rules ([`read_with_reader`]), where such a length
//! raises and no read passes the end; a `pair` with three or more children reads its first two,
//! and an `Array` with three or more reads its elements through
//! `m_Children[1]` (`:216`, `:309`), where UnityPyBoost raises both;
//! an aligned `pair` array reads a child whose type is a key of
//! `FUNCTION_READ_MAP` (every primitive, `string`, `TypelessData`) through
//! that function, without the child's own align flag (a non-empty `string`
//! still aligns itself, as `read_aligned_string` does), and any other child
//! (a class, a vector, a `pair`) through `read_value`, which keeps its flag
//! (`:285-293`); a `ReferencedObject` reads `type` before anything else (a
//! missing key is `KeyError`, `other`; a `type` with no `class`, `ns` or
//! `asm` is `KeyError` and a non-dict `type` `AttributeError`, `:89-97`, both
//! `other` where UnityPyBoost reports `ref_type`) and raises `SerializedFile has no
//! ref_types` for an empty or absent list *before* the empty-class skip (no
//! `ref_types` attribute is an `AttributeError` whose message the dump maps
//! to `ref_type`); an aligned `pair` element with fewer than two children
//! raises `IndexError` (`other`) at every length, since `m_Children[0]` and
//! `[1]` are read before the loop (`:282-283`), while an empty array of
//! non-aligned malformed `pair`s, or of three-child `pair`s, reads as `[]`
//! (the probes: `CANONICAL_DUMP.md` section 7); likewise an aligned vector
//! element whose `Array` has fewer than two children raises `IndexError` at
//! every length, the empty one included, since `m_Children[0].m_Children[1]`
//! is read before the loop (`:309`), where UnityPyBoost reads an empty one
//! as `[]` (`read_value_array` reads an aligned element's `pair` children
//! and its vector's element before its loop, and indexes everywhere else
//! inside it); outside an array, a vector reads its length, and refuses a
//! negative one, before it indexes its `Array`'s element (`:213-216`), and a
//! `pair` reads its first child before it indexes its second (`:194-195`),
//! so a malformed one fails with whatever those reads raise first (a vector
//! whose `Array` has fewer than two children is `negative_length` at length
//! -1 and `IndexError`, `other`, at 0), where UnityPyBoost checks each arity
//! first and reports `other`: this reader never fails a compiled
//! `BadVector` or `BadPair` on entry; and an
//! aligned `ReferencedObject` array element whose class is empty or whose ref
//! type has no tree raises ([`super::ReadDetail::RefTypeNoTree`]). Each
//! difference is an oracle fixture of package B (plan section 7.3).
//!
//! Owner: package B.

use std::sync::Arc;

use bytes::Bytes;

use super::dict::{Dict, FindError, RefIndex, RefNode, Trees, ref_tree, visit};
use super::{
    FloatMode, Out, ReadCtx, ReadDetail, ReadError, ReadErrorKind, RefTypes, Work, charge_error,
    f32_bits, int, limit_refused, stream_refused, uint,
};
use crate::raw::RawStr;
use crate::stream::{Endian, ReaderKind};
use crate::typetree::{Kind, NodeId, Tree};
use crate::value::{ListIx, Parsed, ReadBy, Value, ValueBuilder};

/// Reads one value from `file` by the memory reader's rules (bytes),
/// starting at `start` (any Python position). `bytes_read` counts from
/// `start` and may be negative. Its work is the open's, counted on
/// `ctx.meter` whatever the outcome (the module docs of [`super`], "Work").
pub fn read(
    file: &Bytes,
    start: i128,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
) -> Result<Parsed, ReadError> {
    read_with_reader(file, start, tree, ctx, ReaderKind::Memory)
}

/// [`read`] by the rules of the file's reader: on the stream reader (a file
/// loaded from a path) `start` is a position its `seek` took (never
/// negative), a read stops at the end, a fixed-width read is
/// `unpack(read(n))`, a `TypelessData` length below -1 raises, and an
/// alignment to 2^63 or more raises as the `seek` it makes does
/// ([`crate::stream::ReaderKind`]), so `bytes_read` is never negative.
pub fn read_with_reader(
    file: &Bytes,
    start: i128,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    kind: ReaderKind,
) -> Result<Parsed, ReadError> {
    let work = Work::start(ctx.options.limits, ctx.meter)?;
    let builder =
        ValueBuilder::new(ctx.meter, tree.list().clone()).map_err(|e| charge_error(0, e))?;
    let trees = Trees::new(&builder, tree)?;
    let mut r = Reader {
        data: file,
        bytes: file.as_ref(),
        pos: start,
        start,
        kind,
        big: ctx.endian == Endian::Big,
        floats: ctx.options.floats,
        ref_types: ctx.ref_types,
        refs: RefIndex::new(ctx.ref_types),
        builder,
        trees,
        work,
        has_registry: false,
    };
    let value = r.value(tree, 0, tree.root());
    let at = r.at();
    let ended = r.work.finish(at);
    let value = value?;
    ended?;
    let built = value.into_built(&r.builder, at)?;
    // UnityPy TypeTreeHelper.py:153: `bytes_read = reader.Position - pos`.
    let read = r.pos.saturating_sub(start);
    let bytes_read = i64::try_from(read).unwrap_or(if read < 0 { i64::MIN } else { i64::MAX });
    let Reader { builder, .. } = r;
    Ok(builder
        .finish(built, bytes_read)
        .with_read_by(ReadBy::Python))
}

/// `EndianBinaryReader_Memoryview` over the whole file
/// (`streams/EndianBinaryReader.py:231-284`), or `_Streamable` (`:287-324`)
/// for a file loaded from a path, and `TypeTreeConfig`
/// (`helpers/TypeTreeHelper.py:78-85`).
struct Reader<'a, 'c> {
    data: &'a Bytes,
    bytes: &'a [u8],
    /// `reader.Position`: any Python integer on the memory reader (plan
    /// 3.3); never negative on the stream reader.
    pos: i128,
    start: i128,
    /// Which reader's rules.
    kind: ReaderKind,
    big: bool,
    floats: FloatMode,
    ref_types: RefTypes<'c>,
    /// The ref types' search, one per read ([`RefIndex`]).
    refs: RefIndex<'c>,
    builder: ValueBuilder,
    trees: Trees,
    work: Work<'c>,
    /// `config.has_registry`.
    has_registry: bool,
}

/// A fixed-width number from its bytes in the file's order.
macro_rules! num {
    ($big:expr, $t:ty, $b:expr) => {
        if $big {
            <$t>::from_be_bytes($b)
        } else {
            <$t>::from_le_bytes($b)
        }
    };
}

// Declared after `num!`, which it reads with.
mod array;

/// A Python exception the reader raises.
enum Py {
    /// `struct.error`: an unpack past the buffer, a negative count.
    Struct(u64),
    /// `IndexError` on a node's `m_Children`.
    Index(ReadDetail),
    /// A `TypeError` or `KeyError`/`AttributeError` (kind `other`).
    Other(&'static str),
    /// No Python exception: the open's work passed its allowance.
    Limit(crate::error::LimitError),
}

impl Reader<'_, '_> {
    fn at(&self) -> u64 {
        u64::try_from(self.pos.saturating_sub(self.start)).unwrap_or(0)
    }

    fn fail(&self, kind: ReadErrorKind, detail: ReadDetail) -> ReadError {
        ReadError {
            kind,
            at: self.at(),
            detail,
        }
    }

    fn raise(&self, e: Py) -> ReadError {
        match e {
            // The dump maps none of these messages to a kind of its own.
            Py::Struct(need) => self.fail(ReadErrorKind::Other, ReadDetail::Eof { need }),
            Py::Index(d) => self.fail(ReadErrorKind::Other, d),
            Py::Other(m) => self.fail(ReadErrorKind::Other, ReadDetail::Python(m)),
            Py::Limit(l) => limit_refused(self.at(), l),
        }
    }

    /// `read(size)` by the reader's rules ([`ReaderKind::read`]): the range
    /// it returns, the position moved; the stream reader refuses a size
    /// below -1. A read that takes the position back (the memory reader's
    /// negative size, its -1 past the end) is work the read will repeat
    /// ([`Work::discard`]).
    fn read(&mut self, size: i64) -> Result<std::ops::Range<usize>, ReadError> {
        let (range, next) = self
            .kind
            .read(self.bytes.len(), self.pos, size)
            .map_err(|e| stream_refused(self.at(), e))?;
        if next < self.pos {
            self.work.discard();
        }
        self.pos = next;
        Ok(range)
    }

    /// `unpack(fmt, self.read(N))`, the base class's `read_byte`, `read_int`,
    /// `read_float`... (`EndianBinaryReader.py:93-127`): `FUNCTION_READ_MAP`
    /// holds these base functions (`TypeTreeHelper.py:29-52`), so every
    /// primitive node read through it takes this path. On the memory reader
    /// `read` is a Python slice that moves the position by `N` whatever it
    /// returns, and `unpack` raises unless it has exactly `N` bytes: so at
    /// position `-N` (`view[-N:0]`, empty) it raises where `unpack_from`
    /// reads the last `N` bytes ([`Self::unpack_from`]). On the stream
    /// reader `read` stops at the end.
    fn read_n<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        let range = self.read(i64::try_from(N).unwrap_or(i64::MAX))?;
        self.bytes
            .get(range)
            .and_then(|s| <[u8; N]>::try_from(s).ok())
            .ok_or_else(|| self.raise(Py::Struct(N as u64)))
    }

    /// `read(1)` then `unpack` ([`Self::read_n`]).
    fn read1(&mut self) -> Result<u8, ReadError> {
        let [b] = self.read_n::<1>()?;
        Ok(b)
    }

    /// The reader's own `read_int` of `N` bytes, set on the instance by the
    /// `endian` setter (`EndianBinaryReader.py:73-80`, `:335-349`): the
    /// lengths the reader reads with `reader.read_int()` (a vector's, a
    /// string's, a `TypelessData`'s). On the memory reader
    /// `struct.unpack_from(view, Position)`, the position moving on success
    /// only; on the stream reader `unpack(stream.read(N))`
    /// ([`ReaderKind::unpack`]).
    fn unpack_from<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        let (at, next) = self.kind.unpack(self.bytes.len(), self.pos, N);
        let b = at
            .and_then(|at| self.bytes.get(at..at.checked_add(N)?))
            .and_then(|s| <[u8; N]>::try_from(s).ok());
        let Some(b) = b else {
            let e = self.raise(Py::Struct(N as u64));
            self.pos = next;
            return Err(e);
        };
        self.pos = next;
        Ok(b)
    }

    /// `reader.read_int()`: the instance's [`Self::unpack_from`].
    fn read_int(&mut self) -> Result<i32, ReadError> {
        let big = self.big;
        Ok(num!(big, i32, self.unpack_from()?))
    }

    /// One frame UnityPy's reader pushes on its way down (plan 3.10,
    /// "Bounds"): a `read_value` or `read_value_array` call, a list or dict
    /// comprehension, the lambda a `pair` array reads a non-primitive child
    /// through. Each counts one level against `max_value_depth`, so the count
    /// is the depth of Python frames UnityPy's reader reaches (less the two
    /// or fewer of its leaf reads, `read_int` and the like). Python raises
    /// `RecursionError` past 1,000 frames less its caller's, so every object
    /// it refuses that way passes `max_value_depth` (512) here first.
    fn frame<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<R, ReadError>,
    ) -> Result<R, ReadError> {
        self.work.enter(self.at())?;
        let out = f(self);
        self.work.leave();
        out
    }

    /// `align_stream` (`EndianBinaryReader.py:155-156`): Python's floor modulo
    /// on the file-absolute position, assigned through the `Position`
    /// setter, which the stream reader refuses at 2^63 or more
    /// ([`ReaderKind::align`]).
    fn align(&mut self) -> Result<(), ReadError> {
        self.pos = self
            .kind
            .align(self.pos, 4)
            .map_err(|e| stream_refused(self.at(), e))?;
        Ok(())
    }

    /// `FUNCTION_READ_MAP` (`TypeTreeHelper.py:29-52`) for a primitive
    /// kind: the base class's `unpack(fmt, read(n))` ([`Self::read_n`]).
    fn prim(&mut self, kind: Kind) -> Result<Value, ReadError> {
        let big = self.big;
        Ok(match kind {
            Kind::S8 => int(i64::from(self.read1()?.cast_signed())),
            Kind::U8 => int(i64::from(self.read1()?)),
            Kind::Bool => Value::Bool(self.read1()? != 0),
            Kind::S16 => int(i64::from(num!(big, i16, self.read_n()?))),
            Kind::U16 => int(i64::from(num!(big, u16, self.read_n()?))),
            Kind::S32 => int(i64::from(num!(big, i32, self.read_n()?))),
            Kind::U32 => int(i64::from(num!(big, u32, self.read_n()?))),
            Kind::S64 => int(num!(big, i64, self.read_n()?)),
            Kind::U64 => uint(num!(big, u64, self.read_n()?)),
            // `struct.unpack('f')` widens as UnityPyBoost does (probed: a
            // signalling NaN comes back quiet).
            Kind::F32 => Value::F32(f32_bits(num!(big, u32, self.read_n()?), self.floats)),
            Kind::F64 => Value::F64(num!(big, u64, self.read_n()?)),
            _ => return Err(self.raise(Py::Other("not a primitive"))),
        })
    }

    /// `read_aligned_string` (`EndianBinaryReader.py:266-273`): a length of
    /// 0, or one that is negative or past the end, is `""` and does not align.
    fn string(&mut self) -> Result<Value, ReadError> {
        let len = self.read_int()?;
        let room = i128::try_from(self.bytes.len())
            .unwrap_or(i128::MAX)
            .saturating_sub(self.pos);
        if len > 0 && i128::from(len) <= room {
            let range = self.read(i64::from(len))?;
            let s = RawStr::from_shared(self.data.slice(range));
            self.align()?;
            return Ok(Value::Str(s));
        }
        Ok(Value::Str(RawStr::default()))
    }

    /// `read_byte_array` (`EndianBinaryReader.py:158-159`): `read(read_int())`:
    /// 0 is empty and -1 reads to the end; on the memory reader a Python
    /// slice, so any other negative length moves the position back; on the
    /// stream reader one below -1 raises (`ValueError` "read length must be
    /// non-negative or -1") and a read past the end stops there.
    fn typeless(&mut self) -> Result<Value, ReadError> {
        let len = self.read_int()?;
        let range = self.read(i64::from(len))?;
        Ok(Value::Bytes(self.data.slice(range)))
    }

    fn value(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let at = self.at();
        self.work.enter(at)?;
        let out = self.value_inner(t, lx, node);
        self.work.leave();
        out
    }

    fn child(t: &Tree, node: NodeId, i: usize, missing: ReadDetail) -> Result<NodeId, Py> {
        t.list()
            .children(node)
            .get(i)
            .copied()
            .ok_or(Py::Index(missing))
    }

    /// The vector's `Array` child, when the node reads as one: its first
    /// child's `m_Type` is `"Array"` (`TypeTreeHelper.py:208`, `:306`).
    fn array_child(t: &Tree, node: NodeId) -> Option<NodeId> {
        let first = *t.list().children(node).first()?;
        (t.py_kind(first) == Kind::Array).then_some(first)
    }

    /// `read_value` (`TypeTreeHelper.py:182-263`).
    fn value_inner(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        self.work.values(1, self.at())?;
        let mut align = t.info(node).is_some_and(|i| i.align);
        let kind = t.py_kind(node);
        let out = if kind.prim_size().is_some() {
            Out::Plain(self.prim(kind)?)
        } else if kind == Kind::Str {
            Out::Plain(self.string()?)
        } else if kind == Kind::Bytes {
            Out::Plain(self.typeless()?)
        } else if kind == Kind::Pair {
            // `:193-196`: the first child is read before the second is indexed.
            let a = Self::child(t, node, 0, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let first = self.value(t, lx, a)?;
            let b = Self::child(t, node, 1, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let second = self.value(t, lx, b)?;
            self.tuple(vec_of2(first, second))?
        } else if kind == Kind::ReferencedObject {
            self.ref_object(t, lx, node, false)?
        } else if let Some(array) = Self::array_child(t, node) {
            // `:208-220`.
            align |= t.info(array).is_some_and(|i| i.align);
            let size = self.read_int()?;
            let Ok(count) = usize::try_from(size) else {
                return Err(self.fail(
                    ReadErrorKind::NegativeLength,
                    ReadDetail::NegativeLength(i64::from(size)),
                ));
            };
            let elem =
                Self::child(t, array, 1, ReadDetail::ArrayChildren).map_err(|e| self.raise(e))?;
            if t.info(elem).is_some_and(|i| i.align) {
                self.array(t, lx, elem, i64::from(size))?
            } else {
                // `[read_value(subtype, ...) for _ in range(size)]`: a frame.
                self.frame(|r| r.list(t, lx, elem, count))?
            }
        } else {
            self.class(t, lx, node)?
        };
        if align {
            self.align()?;
        }
        Ok(out)
    }

    fn tuple(&mut self, items: [Out; 2]) -> Result<Out, ReadError> {
        let at = self.at();
        let mut v = self.builder.vec();
        v.reserve_exact(2).map_err(|e| charge_error(at, e))?;
        for item in items {
            item.push_to(&mut v).map_err(|e| charge_error(at, e))?;
        }
        let built = self.builder.tuple(v).map_err(|e| charge_error(at, e))?;
        Ok(Out::Built(built))
    }

    fn remaining(&self) -> usize {
        let len = i128::try_from(self.bytes.len()).unwrap_or(i128::MAX);
        usize::try_from(len.saturating_sub(self.pos).max(0)).unwrap_or(usize::MAX)
    }

    /// The class branch of `read_value` (`:222-231`): children in order, a
    /// `ManagedReferencesRegistry` skipped once one is in scope.
    fn class(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        self.dict(t, lx, node, true)
    }

    /// An aligned class element's `{child.m_Name: read_value(child) ...}`
    /// (`:316-318`), a dict comprehension and so a frame: no registry check.
    fn plain_class(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        self.frame(|r| r.dict(t, lx, node, false))
    }

    /// A class's dict, grown as its children are read ([`Dict`]): no pass
    /// over them first (review round 7, as in UnityPyBoost's reader).
    fn dict(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        registry: bool,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        let is_registry = |c: NodeId| registry && t.py_kind(c) == Kind::ManagedReferencesRegistry;
        let dup = t.info(node).is_some_and(|i| i.dup_names);
        let mut d = Dict::new(&self.builder, kids.len(), dup, at)?;
        let outer = self.has_registry;
        for c in kids {
            visit();
            if is_registry(*c) {
                if self.has_registry {
                    // A skipped child counts as a value (plan 3.10, "Bounds").
                    self.work.skipped(self.at())?;
                    continue;
                }
                // `config = config.copy(); config.has_registry = True`: for
                // the rest of this class only.
                self.has_registry = true;
            }
            let v = self.value(t, lx, *c)?;
            let (key, slot) = t.info(*c).map_or((*c, 0), |i| (i.key, i.slot));
            let at = self.at();
            d.set(&mut self.work, key, slot, v, at)?;
        }
        self.has_registry = outer;
        d.finish(&self.builder, lx, self.at())
    }

    /// A `ReferencedObject` (`:197-206`), or an aligned array's element
    /// (`:294-304`, `in_array`), where a ref type with no node is read as
    /// `read_value(None)` and raises. Its dict grows as its children are
    /// read and keeps each name's position ([`Dict`]), and its `type` is the
    /// entry of the first child named `type` read so far, as in the Boost
    /// reader: no pass over its children first (review round 7).
    fn ref_object(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        in_array: bool,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        let mut d = Dict::new(&self.builder, kids.len(), true, at)?;
        // The slot of the children named `type`, once one is visited.
        let mut type_slot = None;
        // The current `type`'s resolution, kept until a child sets `type`
        // again, as the Boost reader keeps it.
        let mut resolved: Option<Option<RefNode>> = None;
        for c in kids {
            visit();
            let (key, slot) = t.info(*c).map_or((*c, 0), |i| (i.key, i.slot));
            if type_slot.is_none() && t.info(*c).is_some() && t.list().name(*c) == b"type" {
                type_slot = Some(slot);
            }
            let v = if t.py_kind(*c) == Kind::ReferencedObjectData {
                let node = if let Some(r) = &resolved {
                    r.clone()
                } else {
                    let ty = type_slot
                        .and_then(|s| d.position(s))
                        .and_then(|p| d.entries.as_slice().get(p))
                        .map(|(_, v)| v);
                    resolved.insert(self.ref_node(ty)?).clone()
                };
                match (node, in_array) {
                    (Some(RefNode::Tree(rt)), _) => {
                        let at = self.at();
                        let rlx = self.trees.index(&mut self.builder, &rt, at)?;
                        self.value(&rt, rlx, rt.root())?
                    }
                    (None | Some(RefNode::NoTree), false) => {
                        // A skipped child counts as a value (plan 3.10, "Bounds").
                        self.work.skipped(self.at())?;
                        continue;
                    }
                    (None | Some(RefNode::NoTree), true) => {
                        return Err(self.fail(ReadErrorKind::Other, ReadDetail::RefTypeNoTree));
                    }
                }
            } else {
                self.value(t, lx, *c)?
            };
            let at = self.at();
            d.set(&mut self.work, key, slot, v, at)?;
            if Some(slot) == type_slot {
                resolved = None;
            }
        }
        d.finish(&self.builder, lx, self.at())
    }

    /// `get_ref_type_node` (`TypeTreeHelper.py:88-109`) on the dict read so
    /// far: `type` first (`KeyError`), then its `class`, `ns` and `asm`
    /// (`KeyError`, or `AttributeError` when `type` is not a dict), then
    /// `ref_types` (absent or empty: `ref_type`), then the empty-class skip,
    /// then the scan.
    fn ref_node(&mut self, ty: Option<&Value>) -> Result<Option<RefNode>, ReadError> {
        let Some(ty) = ty else {
            return Err(self.raise(Py::Other("KeyError: 'type'")));
        };
        let Value::Dict(m) = ty else {
            return Err(self.raise(Py::Other("AttributeError: no attribute 'class'")));
        };
        let at = self.at();
        let mut field = |name: &'static [u8], msg: &'static str| match self.trees.field(
            m,
            name,
            &mut self.work.open,
        ) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err(Py::Other(msg)),
            Err(l) => Err(Py::Limit(l)),
        };
        let fields = (|| {
            Ok((
                field(b"class", "KeyError: 'class'")?,
                field(b"ns", "KeyError: 'ns'")?,
                field(b"asm", "KeyError: 'asm'")?,
            ))
        })();
        let (cls, ns, asm) = fields.map_err(|e| match e {
            Py::Limit(l) => limit_refused(at, l),
            e => self.raise(e),
        })?;
        if !matches!(self.ref_types, RefTypes::List(l) if !l.is_empty()) {
            return Err(self.fail(ReadErrorKind::RefType, ReadDetail::NoRefTypes));
        }
        if matches!(cls, Value::Str(s) if s.is_empty()) {
            return Ok(None);
        }
        let found = self
            .refs
            .find(self.builder.meter(), &mut self.work.open, cls, ns, asm)
            .map_err(|e| match e {
                FindError::Charge(e) => charge_error(at, e),
                FindError::Work(l) => limit_refused(at, l),
            })?;
        match found {
            Some(r) => Ok(Some(ref_tree(r, &self.work.limits, at)?)),
            None => Err(self.fail(ReadErrorKind::RefType, ReadDetail::RefTypeNotFound)),
        }
    }
}

fn vec_of2(a: Out, b: Out) -> [Out; 2] {
    [a, b]
}
