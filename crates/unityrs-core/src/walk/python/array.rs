//! The pure-Python reader's arrays: a vector's elements read one by one
//! (`read_value`'s list comprehension, a primitive element's list held as a
//! run) and `read_value_array` for an aligned element, with its `struct`
//! formats.

use std::sync::Arc;

use super::{Py, Reader};
use crate::limits::MeteredVec;
use crate::stream::{py_read, py_unpack_from};
use crate::typetree::{Kind, NodeId, Tree};
use crate::value::{ListIx, PrimArray, PrimRun, Value};
use crate::walk::{
    Out, ReadDetail, ReadError, charge_error, f32_bits, first_reserve, grow_step, int, uint,
};

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

impl Reader<'_, '_> {
    /// `[read_value(subtype) for _ in range(size)]` (`:220`): a list of an
    /// element read one by one; a primitive element's list is held as a run
    /// (the same Python list of numbers), `vector<UInt8>`'s as a slice.
    pub(super) fn list(
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
            grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
            let before = self.pos;
            let v = self.value(t, lx, elem)?;
            if self.pos <= before {
                self.work.zero_size(self.at())?;
            }
            v.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// A list of primitives read one by one (`read_value` of a primitive
    /// element, unflagged, so never aligned).
    #[inline(never)]
    fn prim_list(&mut self, kind: Kind, count: usize) -> Result<Out, ReadError> {
        let size = kind.prim_size().unwrap_or(1);
        if kind == Kind::U8 {
            // Each `read(1)` is the next byte: a valid run is one slice
            // (crossing position -1 is an empty read, which raises).
            let mut first = None;
            for i in 0..count {
                self.work.run(1, 1, self.at())?;
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
                    self.work.run(1, u64::from(size), self.at())?;
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
                .read_n()
                .map(|b| num!(big, u16, b))),
            Kind::S16 => run!(i16, PrimRun::I16, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, i16, b))),
            Kind::U32 => run!(u32, PrimRun::U32, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, u32, b))),
            Kind::S32 => run!(i32, PrimRun::I32, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, i32, b))),
            Kind::U64 => run!(u64, PrimRun::U64, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, u64, b))),
            Kind::S64 => run!(i64, PrimRun::I64, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, i64, b))),
            Kind::F32 => run!(u32, PrimRun::F32, |r: &mut Self| r
                .read_n()
                .map(|b| f32_bits(num!(big, u32, b), floats))),
            _ => run!(u64, PrimRun::F64, |r: &mut Self| r
                .read_n()
                .map(|b| num!(big, u64, b))),
        }
    }

    /// `read_array_struct` (`EndianBinaryReader.py:258-264`): a `struct`
    /// format of `size` items, refused for a negative count, unpacked from
    /// the position in one piece; a tuple.
    #[inline(never)]
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
    /// length is not checked, `:311-313`): a frame.
    pub(super) fn array(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        size: i64,
    ) -> Result<Out, ReadError> {
        self.frame(|r| r.array_inner(t, lx, node, size))
    }

    /// The body of [`Self::array`]. Each list comprehension it reads through
    /// is a frame of its own ([`Self::comprehension`]); its loop over
    /// `ReferencedObject` elements is not.
    fn array_inner(
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
            self.comprehension(count, 4, |r| r.string().map(Out::Plain))?
        } else if kind == Kind::Bytes {
            // `:280`: `read_bytes()` without its length, once there is an element.
            if count > 0 {
                return Err(self.raise(Py::Other(
                    "TypeError: read_bytes() missing 1 required positional argument",
                )));
            }
            self.comprehension(0, 4, |r| r.typeless().map(Out::Plain))?
        } else if kind == Kind::Pair {
            // `:282-293`: both children before the loop; each read without
            // its own flag when its type is in `FUNCTION_READ_MAP`.
            let a = Self::child(t, node, 0, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let b = Self::child(t, node, 1, ReadDetail::PairChildren).map_err(|e| self.raise(e))?;
            let min = t.info(node).map_or(0, |i| i.min_size);
            self.comprehension(count, min, |r| {
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
                self.comprehension(count, 4, |r| {
                    let inner = r.read_int()?;
                    r.array(t, lx, sub, i64::from(inner))
                })?
            } else {
                // `[[read_value(subtype) for _ in range(reader.read_int())]
                // for _ in range(size)]`: the inner comprehension too.
                self.comprehension(count, 4, |r| {
                    let inner = r.read_int()?;
                    let n = usize::try_from(inner).unwrap_or(0);
                    r.frame(|r| r.list(t, lx, sub, n))
                })?
            }
        } else {
            // `:314-318`: classes in a dict comprehension, with no registry check.
            let min = t.info(node).map_or(0, |i| i.min_size);
            self.comprehension(count, min, |r| r.plain_class(t, lx, node))?
        };
        if align {
            self.align();
        }
        Ok(out)
    }

    /// `[item() for _ in range(count)]`: [`Self::each`] in a frame.
    fn comprehension(
        &mut self,
        count: usize,
        min: u32,
        item: impl FnMut(&mut Self) -> Result<Out, ReadError>,
    ) -> Result<Out, ReadError> {
        self.frame(|r| r.each(count, min, item))
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
            self.work.values(1, self.at())?;
            grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
            let before = self.pos;
            let v = item(self)?;
            if self.pos <= before {
                self.work.zero_size(self.at())?;
            }
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
            // `lambda reader: read_value(child, reader, config)`: a frame.
            self.frame(|r| r.value(t, lx, node))
        }
    }
}
