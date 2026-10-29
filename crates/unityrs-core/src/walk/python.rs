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
//! be negative); a `pair` with three or more children reads its first two,
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

use super::dict::{Dict, RefNode, Trees, names_match, ref_tree, slot_count, slot_of};
use super::{
    FloatMode, Out, ReadCtx, ReadDetail, ReadError, ReadErrorKind, RefTypes, Work, charge_error,
    f32_bits, first_reserve, grow_step, int, uint,
};
use crate::limits::MeteredVec;
use crate::raw::RawStr;
use crate::stream::{Endian, py_align, py_read, py_unpack_from};
use crate::typetree::{Kind, NodeId, Tree};
use crate::value::{ListIx, Parsed, PrimArray, PrimRun, Value, ValueBuilder};

/// Reads one value from `file`, starting at `start` (any Python position).
/// `bytes_read` counts from `start` and may be negative.
pub fn read(
    file: &Bytes,
    start: i128,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
) -> Result<Parsed, ReadError> {
    let builder =
        ValueBuilder::new(ctx.meter, tree.list().clone()).map_err(|e| charge_error(0, e))?;
    let trees = Trees::new(&builder, tree)?;
    let scratch = builder.vec();
    let mut r = Reader {
        data: file,
        bytes: file.as_ref(),
        pos: start,
        start,
        big: ctx.endian == Endian::Big,
        floats: ctx.options.floats,
        ref_types: ctx.ref_types,
        builder,
        trees,
        work: Work::new(ctx.options.limits),
        has_registry: false,
        scratch,
    };
    let value = r.value(tree, 0, tree.root())?;
    let at = r.at();
    let built = value.into_built(&r.builder, at)?;
    // UnityPy TypeTreeHelper.py:153: `bytes_read = reader.Position - pos`.
    let read = r.pos.saturating_sub(start);
    let bytes_read = i64::try_from(read).unwrap_or(if read < 0 { i64::MIN } else { i64::MAX });
    let Reader { builder, .. } = r;
    Ok(builder.finish(built, bytes_read))
}

/// `EndianBinaryReader_Memoryview` over the whole file
/// (`streams/EndianBinaryReader.py:231-284`) and `TypeTreeConfig`
/// (`helpers/TypeTreeHelper.py:78-85`).
struct Reader<'a, 'c> {
    data: &'a Bytes,
    bytes: &'a [u8],
    /// `reader.Position`: any Python integer (plan 3.3).
    pos: i128,
    start: i128,
    big: bool,
    floats: FloatMode,
    ref_types: RefTypes<'c>,
    builder: ValueBuilder,
    trees: Trees,
    work: Work,
    /// `config.has_registry`.
    has_registry: bool,
    scratch: MeteredVec<u32>,
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

/// `struct` formats of `read_array_struct` (`EndianBinaryReader.py:170-198`).
#[derive(Clone, Copy)]
enum Fmt {
    Bool,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Fmt {
    fn size(self) -> usize {
        match self {
            Self::Bool | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }

    /// `FUNCTION_READ_MAP_ARRAY`'s format for a kind (`TypeTreeHelper.py:54-75`):
    /// `read_short_array` unpacks `H` and `read_u_short_array` `h`
    /// (`EndianBinaryReader.py:176-180`), so 16-bit signedness is swapped.
    /// `None` for `SInt8`, whose `read_byte_array` takes no length.
    fn of_array(kind: Kind) -> Option<Self> {
        Some(match kind {
            Kind::U8 => Self::U8,
            Kind::S16 => Self::U16,
            Kind::U16 => Self::I16,
            Kind::S32 => Self::I32,
            Kind::U32 => Self::U32,
            Kind::S64 => Self::I64,
            Kind::U64 => Self::U64,
            Kind::F32 => Self::F32,
            Kind::F64 => Self::F64,
            Kind::Bool => Self::Bool,
            _ => return None,
        })
    }
}

/// A Python exception the reader raises.
enum Py {
    /// `struct.error`: an unpack past the buffer, a negative count.
    Struct(u64),
    /// `IndexError` on a node's `m_Children`.
    Index(ReadDetail),
    /// A `TypeError` or `KeyError`/`AttributeError` (kind `other`).
    Other(&'static str),
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
        }
    }

    /// `read(1)` then `unpack` (`EndianBinaryReader.py:93-97`, `:126-127`):
    /// the position moves even when the slice is short and `unpack` raises.
    fn read1(&mut self) -> Result<u8, ReadError> {
        let (range, next) = py_read(self.bytes.len(), self.pos, 1);
        self.pos = next;
        match self.bytes.get(range) {
            Some([b]) => Ok(*b),
            _ => Err(self.raise(Py::Struct(1))),
        }
    }

    /// `struct.unpack_from(view, Position)` of `N` bytes
    /// (`EndianBinaryReader.py:340-343`): the position moves on success only.
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        let b = py_unpack_from(self.bytes.len(), self.pos, N)
            .and_then(|at| self.bytes.get(at..at.checked_add(N)?))
            .and_then(|s| <[u8; N]>::try_from(s).ok())
            .ok_or_else(|| self.raise(Py::Struct(N as u64)))?;
        self.pos = self.pos.saturating_add(N as i128);
        Ok(b)
    }

    fn read_int(&mut self) -> Result<i32, ReadError> {
        let big = self.big;
        Ok(num!(big, i32, self.fixed()?))
    }

    /// `align_stream` (`EndianBinaryReader.py:155-156`): Python's floor modulo
    /// on the file-absolute position.
    fn align(&mut self) {
        self.pos = py_align(self.pos, 4);
    }

    /// `FUNCTION_READ_MAP` (`TypeTreeHelper.py:29-52`) for a primitive kind.
    fn prim(&mut self, kind: Kind) -> Result<Value, ReadError> {
        let big = self.big;
        Ok(match kind {
            Kind::S8 => int(i64::from(self.read1()?.cast_signed())),
            Kind::U8 => int(i64::from(self.read1()?)),
            Kind::Bool => Value::Bool(self.read1()? != 0),
            Kind::S16 => int(i64::from(num!(big, i16, self.fixed()?))),
            Kind::U16 => int(i64::from(num!(big, u16, self.fixed()?))),
            Kind::S32 => int(i64::from(num!(big, i32, self.fixed()?))),
            Kind::U32 => int(i64::from(num!(big, u32, self.fixed()?))),
            Kind::S64 => int(num!(big, i64, self.fixed()?)),
            Kind::U64 => uint(num!(big, u64, self.fixed()?)),
            // `struct.unpack('f')` widens as UnityPyBoost does (probed: a
            // signalling NaN comes back quiet).
            Kind::F32 => Value::F32(f32_bits(num!(big, u32, self.fixed()?), self.floats)),
            Kind::F64 => Value::F64(num!(big, u64, self.fixed()?)),
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
            let (range, next) = py_read(self.bytes.len(), self.pos, i64::from(len));
            self.pos = next;
            let s = RawStr::from_shared(self.data.slice(range));
            self.align();
            return Ok(Value::Str(s));
        }
        Ok(Value::Str(RawStr::default()))
    }

    /// `read_byte_array` (`EndianBinaryReader.py:158-159`): `read(read_int())`,
    /// a Python slice: 0 is empty, -1 reads to the end, any other negative
    /// length moves the position back.
    fn typeless(&mut self) -> Result<Value, ReadError> {
        let len = self.read_int()?;
        let (range, next) = py_read(self.bytes.len(), self.pos, i64::from(len));
        self.pos = next;
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
                self.list(t, lx, elem, count)?
            }
        } else {
            self.class(t, lx, node)?
        };
        if align {
            self.align();
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

    /// `[read_value(subtype) for _ in range(size)]` (`:220`): a list of an
    /// element read one by one; a primitive element's list is held as a run
    /// (the same Python list of numbers), `vector<UInt8>`'s as a slice.
    fn list(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        elem: NodeId,
        count: usize,
    ) -> Result<Out, ReadError> {
        let kind = t.py_kind(elem);
        let ei = t.info(elem).copied();
        let min = ei.map_or(0, |i| i.min_size);
        if kind.prim_size().is_some() {
            return self.prim_list(kind, count);
        }
        let at = self.at();
        let max = self.work.limits.max_prealloc;
        let mut items = self.builder.vec();
        items
            .reserve_exact(first_reserve(count, self.remaining(), min, max))
            .map_err(|e| charge_error(at, e))?;
        for _ in 0..count {
            if min == 0 {
                self.work.zero_size(self.at())?;
            }
            grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
            let v = self.value(t, lx, elem)?;
            v.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// A list of primitives read one by one (`read_value` of a primitive
    /// element, unflagged, so never aligned).
    fn prim_list(&mut self, kind: Kind, count: usize) -> Result<Out, ReadError> {
        let size = kind.prim_size().unwrap_or(1);
        if kind == Kind::U8 {
            // Each `read(1)` is the next byte: a valid run is one slice
            // (crossing position -1 is an empty read, which raises).
            let mut first = None;
            for i in 0..count {
                self.work.values(1, self.at())?;
                let (range, next) = py_read(self.bytes.len(), self.pos, 1);
                if range.len() != 1 || first.is_some_and(|f: usize| f + i != range.start) {
                    self.pos = next;
                    return Err(self.raise(Py::Struct(1)));
                }
                first.get_or_insert(range.start);
                self.pos = next;
            }
            let s = first.unwrap_or(0);
            let b = self
                .data
                .slice(s..s + if first.is_some() { count } else { 0 });
            return Ok(Out::Plain(Value::Prims(PrimArray::U8(b))));
        }
        let floats = self.floats;
        macro_rules! run {
            ($t:ty, $wrap:expr, $get:expr) => {{
                let at = self.at();
                let max = self.work.limits.max_prealloc;
                let mut items: MeteredVec<$t> = self.builder.vec();
                items
                    .reserve_exact(first_reserve(count, self.remaining(), size, max))
                    .map_err(|e| charge_error(at, e))?;
                for _ in 0..count {
                    self.work.values(1, self.at())?;
                    grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
                    let v: $t = $get(self)?;
                    items.push(v).map_err(|e| charge_error(self.at(), e))?;
                }
                let built = self
                    .builder
                    .prims($wrap(items))
                    .map_err(|e| charge_error(at, e))?;
                Ok(Out::Built(built))
            }};
        }
        let big = self.big;
        match kind {
            Kind::Bool => run!(bool, PrimRun::Bool, |r: &mut Self| r
                .read1()
                .map(|b| b != 0)),
            Kind::S8 => run!(i8, PrimRun::I8, |r: &mut Self| r
                .read1()
                .map(u8::cast_signed)),
            Kind::U16 => run!(u16, PrimRun::U16, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, u16, b))),
            Kind::S16 => run!(i16, PrimRun::I16, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, i16, b))),
            Kind::U32 => run!(u32, PrimRun::U32, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, u32, b))),
            Kind::S32 => run!(i32, PrimRun::I32, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, i32, b))),
            Kind::U64 => run!(u64, PrimRun::U64, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, u64, b))),
            Kind::S64 => run!(i64, PrimRun::I64, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, i64, b))),
            Kind::F32 => run!(u32, PrimRun::F32, |r: &mut Self| r
                .fixed()
                .map(|b| f32_bits(num!(big, u32, b), floats))),
            _ => run!(u64, PrimRun::F64, |r: &mut Self| r
                .fixed()
                .map(|b| num!(big, u64, b))),
        }
    }

    /// `read_array_struct` (`EndianBinaryReader.py:258-264`): a `struct`
    /// format of `size` items, refused for a negative count, unpacked from
    /// the position in one piece; a tuple.
    fn struct_tuple(&mut self, fmt: Fmt, size: i64) -> Result<Out, ReadError> {
        let Ok(count) = usize::try_from(size) else {
            return Err(self.raise(Py::Other("struct.error: bad char in struct format")));
        };
        let n = count
            .checked_mul(fmt.size())
            .ok_or_else(|| self.raise(Py::Struct(u64::MAX)))?;
        let at = py_unpack_from(self.bytes.len(), self.pos, n)
            .ok_or_else(|| self.raise(Py::Struct(n as u64)))?;
        let bytes = self.bytes.get(at..at.saturating_add(n)).unwrap_or_default();
        self.work.values(count as u64, self.at())?;
        let here = self.at();
        let mut items = self.builder.vec();
        items
            .reserve_exact(count)
            .map_err(|e| charge_error(here, e))?;
        let big = self.big;
        for c in bytes.chunks_exact(fmt.size()) {
            let mut b = [0_u8; 8];
            if let Some(dst) = b.get_mut(..c.len()) {
                dst.copy_from_slice(c);
            }
            let [b0, b1, b2, b3, b4, b5, b6, b7] = b;
            let v = match fmt {
                Fmt::Bool => Value::Bool(b0 != 0),
                Fmt::U8 => int(i64::from(b0)),
                Fmt::I16 => int(i64::from(num!(big, i16, [b0, b1]))),
                Fmt::U16 => int(i64::from(num!(big, u16, [b0, b1]))),
                Fmt::I32 => int(i64::from(num!(big, i32, [b0, b1, b2, b3]))),
                Fmt::U32 => int(i64::from(num!(big, u32, [b0, b1, b2, b3]))),
                Fmt::F32 => Value::F32(f32_bits(num!(big, u32, [b0, b1, b2, b3]), self.floats)),
                Fmt::I64 => int(num!(big, i64, b)),
                Fmt::U64 => uint(num!(big, u64, b)),
                Fmt::F64 => Value::F64(num!(big, u64, [b0, b1, b2, b3, b4, b5, b6, b7])),
            };
            items.push(v).map_err(|e| charge_error(here, e))?;
        }
        self.pos = self.pos.saturating_add(n as i128);
        let built = self
            .builder
            .tuple(items)
            .map_err(|e| charge_error(here, e))?;
        Ok(Out::Built(built))
    }

    /// `read_value_array` (`TypeTreeHelper.py:266-361`) for an aligned
    /// element `node` and a `size` that may be negative (an inner vector's
    /// length is not checked, `:311-313`).
    fn array(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        size: i64,
    ) -> Result<Out, ReadError> {
        let mut align = t.info(node).is_some_and(|i| i.align);
        let kind = t.py_kind(node);
        // `range(size)` of a negative size is empty.
        let count = usize::try_from(size).unwrap_or(0);
        let out = if kind == Kind::S8 {
            // `:55`: `read_byte_array(reader, size)` takes no length.
            return Err(self.raise(Py::Other(
                "TypeError: read_byte_array() takes 1 positional argument",
            )));
        } else if let Some(fmt) = Fmt::of_array(kind) {
            self.struct_tuple(fmt, size)?
        } else if kind == Kind::Str {
            self.each(count, 4, |r| r.string().map(Out::Plain))?
        } else if kind == Kind::Bytes {
            // `:280`: `read_bytes()` without its length, once there is an element.
            if count > 0 {
                return Err(self.raise(Py::Other(
                    "TypeError: read_bytes() missing 1 required positional argument",
                )));
            }
            self.each(0, 4, |r| r.typeless().map(Out::Plain))?
        } else if kind == Kind::Pair {
            // `:282-293`: both children before the loop; each read without
            // its own flag when its type is in `FUNCTION_READ_MAP`.
            let a = Self::child(t, node, 0, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let b = Self::child(t, node, 1, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let min = t.info(node).map_or(0, |i| i.min_size);
            self.each(count, min, |r| {
                let first = r.direct(t, lx, a)?;
                let second = r.direct(t, lx, b)?;
                r.tuple([first, second])
            })?
        } else if kind == Kind::ReferencedObject {
            let min = t.info(node).map_or(0, |i| i.min_size);
            self.each(count, min, |r| r.ref_object(t, lx, node, true))?
        } else if let Some(arr) = Self::array_child(t, node) {
            // `:306-313`: the element's `Array` element is indexed before the loop.
            align |= t.info(arr).is_some_and(|i| i.align);
            let sub =
                Self::child(t, arr, 1, ReadDetail::ArrayChildren).map_err(|e| self.raise(e))?;
            if t.info(sub).is_some_and(|i| i.align) {
                self.each(count, 4, |r| {
                    let inner = r.read_int()?;
                    r.array(t, lx, sub, i64::from(inner))
                })?
            } else {
                self.each(count, 4, |r| {
                    let inner = r.read_int()?;
                    let n = usize::try_from(inner).unwrap_or(0);
                    r.list(t, lx, sub, n)
                })?
            }
        } else {
            // `:314-318`: classes in a dict comprehension, with no registry check.
            let min = t.info(node).map_or(0, |i| i.min_size);
            self.each(count, min, |r| r.plain_class(t, lx, node))?
        };
        if align {
            self.align();
        }
        Ok(out)
    }

    /// A list of `count` items, each read by `item`.
    fn each(
        &mut self,
        count: usize,
        min: u32,
        mut item: impl FnMut(&mut Self) -> Result<Out, ReadError>,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let max = self.work.limits.max_prealloc;
        let mut items = self.builder.vec();
        items
            .reserve_exact(first_reserve(count, self.remaining(), min, max))
            .map_err(|e| charge_error(at, e))?;
        for _ in 0..count {
            if min == 0 {
                self.work.zero_size(self.at())?;
            }
            self.work.values(1, self.at())?;
            grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
            let v = item(self)?;
            v.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// `FUNCTION_READ_MAP.get(child.m_Type, lambda: read_value(child))`
    /// (`:285-292`): a primitive, `string` or `TypelessData` child is read by
    /// its function, without its own align flag; any other through `read_value`.
    fn direct(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let kind = t.py_kind(node);
        if kind.prim_size().is_some() {
            self.work.values(1, self.at())?;
            Ok(Out::Plain(self.prim(kind)?))
        } else if kind == Kind::Str {
            self.work.values(1, self.at())?;
            Ok(Out::Plain(self.string()?))
        } else if kind == Kind::Bytes {
            self.work.values(1, self.at())?;
            Ok(Out::Plain(self.typeless()?))
        } else {
            self.value(t, lx, node)
        }
    }

    /// The class branch of `read_value` (`:222-231`): children in order, a
    /// `ManagedReferencesRegistry` skipped once one is in scope.
    fn class(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        self.dict(t, lx, node, true)
    }

    /// An aligned class element's `{child.m_Name: read_value(child) ...}`
    /// (`:316-318`): no registry check.
    fn plain_class(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        self.work.enter(self.at())?;
        let out = self.dict(t, lx, node, false);
        self.work.leave();
        out
    }

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
        // The entries it will set: the distinct names of the children it reads.
        let mut reg = self.has_registry;
        let width = slot_count(t, node);
        let dup = t.info(node).is_some_and(|i| i.dup_names);
        let mut seen = self.builder.vec::<bool>();
        if dup {
            seen.reserve_exact(width).map_err(|e| charge_error(at, e))?;
            for _ in 0..width {
                seen.push(false).map_err(|e| charge_error(at, e))?;
            }
        }
        let mut reserve = 0;
        for c in kids {
            if is_registry(*c) {
                if reg {
                    continue;
                }
                reg = true;
            }
            let slot = t.info(*c).map_or(0, |i| i.slot as usize);
            if !dup {
                reserve += 1;
            } else if let Some(s) = seen.as_mut_slice().get_mut(slot)
                && !*s
            {
                *s = true;
                reserve += 1;
            }
        }
        drop(seen);
        let mut d = Dict::new(
            &self.builder,
            &mut self.scratch,
            reserve,
            if dup { width } else { 0 },
            at,
        )?;
        let outer = self.has_registry;
        for c in kids {
            if is_registry(*c) {
                if self.has_registry {
                    continue;
                }
                // `config = config.copy(); config.has_registry = True`: for
                // the rest of this class only.
                self.has_registry = true;
            }
            let v = self.value(t, lx, *c)?;
            let (key, slot) = t.info(*c).map_or((*c, 0), |i| (i.key, i.slot));
            let at = self.at();
            d.set(&mut self.scratch, key, slot, v, at)?;
        }
        self.has_registry = outer;
        Ok(d.finish(&self.builder, &mut self.scratch, lx))
    }

    /// A `ReferencedObject` (`:197-206`), or an aligned array's element
    /// (`:294-304`, `in_array`), where a ref type with no node is read as
    /// `read_value(None)` and raises.
    fn ref_object(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        in_array: bool,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        let width = slot_count(t, node);
        let type_slot = slot_of(t, node, b"type");
        let mut d = Dict::new(&self.builder, &mut self.scratch, 0, width, at)?;
        for c in kids {
            let v = if t.py_kind(*c) == Kind::ReferencedObjectData {
                let ty = type_slot
                    .and_then(|s| d.position(&self.scratch, s))
                    .and_then(|p| d.entries.as_slice().get(p))
                    .map(|(_, v)| v);
                match (self.ref_node(ty)?, in_array) {
                    (Some(RefNode::Tree(rt)), _) => {
                        let at = self.at();
                        let rlx = self.trees.index(&mut self.builder, &rt, at)?;
                        self.value(&rt, rlx, rt.root())?
                    }
                    (None | Some(RefNode::NoTree), false) => continue,
                    (None | Some(RefNode::NoTree), true) => {
                        return Err(self.fail(ReadErrorKind::Other, ReadDetail::RefTypeNoTree));
                    }
                }
            } else {
                self.value(t, lx, *c)?
            };
            let (key, slot) = t.info(*c).map_or((*c, 0), |i| (i.key, i.slot));
            let at = self.at();
            d.set(&mut self.scratch, key, slot, v, at)?;
        }
        Ok(d.finish(&self.builder, &mut self.scratch, lx))
    }

    /// `get_ref_type_node` (`TypeTreeHelper.py:88-109`) on the dict read so
    /// far: `type` first (`KeyError`), then its `class`, `ns` and `asm`
    /// (`KeyError`, or `AttributeError` when `type` is not a dict), then
    /// `ref_types` (absent or empty: `ref_type`), then the empty-class skip,
    /// then the scan.
    fn ref_node(&self, ty: Option<&Value>) -> Result<Option<RefNode>, ReadError> {
        let Some(ty) = ty else {
            return Err(self.raise(Py::Other("KeyError: 'type'")));
        };
        let Value::Dict(m) = ty else {
            return Err(self.raise(Py::Other("AttributeError: no attribute 'class'")));
        };
        let field = |name: &'static [u8], msg: &'static str| {
            self.trees
                .field(m, name)
                .ok_or_else(|| self.raise(Py::Other(msg)))
        };
        let cls = field(b"class", "KeyError: 'class'")?;
        let ns = field(b"ns", "KeyError: 'ns'")?;
        let asm = field(b"asm", "KeyError: 'asm'")?;
        let list = match self.ref_types {
            RefTypes::List(l) if !l.is_empty() => l,
            _ => return Err(self.fail(ReadErrorKind::RefType, ReadDetail::NoRefTypes)),
        };
        if matches!(cls, Value::Str(s) if s.is_empty()) {
            return Ok(None);
        }
        match list.iter().find(|r| names_match(r, cls, ns, asm)) {
            Some(r) => Ok(Some(ref_tree(r, &self.work.limits, self.at())?)),
            None => Err(self.fail(ReadErrorKind::RefType, ReadDetail::RefTypeNotFound)),
        }
    }
}

fn vec_of2(a: Out, b: Out) -> [Out; 2] {
    [a, b]
}
