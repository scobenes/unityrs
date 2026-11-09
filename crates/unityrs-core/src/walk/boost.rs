// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! UnityPyBoost's reader (`UnityPyBoost/TypeTreeHelper.cpp:67-1056`), for
//! objects with `byte_size > 0`. The rules, each pinned by a test (plan
//! section 3.10, R1-R16): bounds checks `pos + n > end`, a length's before
//! the bytes it counts; `align4` from the object's start with no bounds check; strings
//! always aligned; `TypelessData` aligned only by its flag; the switch on the
//! node's own kind first, and only a kind it does not handle (`Unknown`,
//! `PPtr`, `Array`, `ReferencedObjectData`, `ManagedReferencesRegistry`) is an
//! array when its first child is `Array` (R9); primitive and `pair` elements
//! read as one run and align once, a `pair` run checking its arity before the
//! first element, so even when empty; other elements read and align one by
//! one; the `ManagedReferencesRegistry` scope; `ReferencedObject` data
//! resolved through the file's ref types in UnityPyBoost's order (R14: no
//! file, `ref_types` absent or not a list, no `type` read yet, a missing
//! `class`/`ns`/`asm`, all `ref_type`; then an empty class or a matched ref
//! type with no tree skips `data`; no match is `ref_type`; a `class` that is
//! not a `Str` skips `data` when its Python object's size field is 0, else
//! is `ref_type`, plan R14); `PyDict_SetItem`
//! for repeated names; float32 widened (a signalling NaN quieted under
//! [`super::FloatMode::UnityPy`]).
//!
//! Owner: package B.

use std::sync::Arc;

use bytes::Bytes;

use super::dict::{Dict, FindError, RefIndex, RefNode, Trees, ref_tree, visit};
use super::{
    FloatMode, Out, ReadCtx, ReadDetail, ReadError, ReadErrorKind, RefTypes, Work, charge_error,
    f32_bits, grow_step, int, limit_refused, py_size_field_is_zero, uint,
};
use crate::limits::MeteredVec;
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{Kind, NodeId, NodeInfo, Shape, Tree};
use crate::value::{ListIx, Parsed, PrimArray, PrimRun, Value, ValueBuilder};

/// Reads one value from exactly `data` (the object's bytes). `TypelessData`
/// and `vector<UInt8>` values are zero-copy slices of `data`.
/// Its work is the open's, counted on `ctx.meter` whatever the outcome
/// (the module docs of [`super`], "Work").
pub fn read(data: &Bytes, tree: &Arc<Tree>, ctx: &ReadCtx<'_>) -> Result<Parsed, ReadError> {
    let work = Work::start(ctx.options.limits, ctx.meter)?;
    let builder =
        ValueBuilder::new(ctx.meter, tree.list().clone()).map_err(|e| charge_error(0, e))?;
    let trees = Trees::new(&builder, tree)?;
    let mut r = Reader {
        data,
        bytes: data.as_ref(),
        pos: 0,
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
    // UnityPy TypeTreeHelper.cpp:1045: `bytes_read = reader.ptr - reader.start`.
    let bytes_read = i64::try_from(r.pos).unwrap_or(i64::MAX);
    let Reader { builder, .. } = r;
    Ok(builder.finish(built, bytes_read))
}

/// UnityPyBoost's `ReaderT` (`TypeTreeHelper.cpp:13-18`) and its config
/// (`:20-26`), with the reader's own bounds on work.
struct Reader<'a, 'c> {
    data: &'a Bytes,
    bytes: &'a [u8],
    /// `ptr - start`: may pass the end after an alignment (R3).
    pos: usize,
    big: bool,
    floats: FloatMode,
    ref_types: RefTypes<'c>,
    /// The ref types' search, one per read ([`RefIndex`]).
    refs: RefIndex<'c>,
    builder: ValueBuilder,
    trees: Trees,
    work: Work<'c>,
    /// `config->has_registry` (R13).
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

impl Reader<'_, '_> {
    fn at(&self) -> u64 {
        u64::try_from(self.pos).unwrap_or(u64::MAX)
    }

    fn fail(&self, kind: ReadErrorKind, detail: ReadDetail) -> ReadError {
        ReadError {
            kind,
            at: self.at(),
            detail,
        }
    }

    /// A read past the end (`EOFError`, or a `ValueError` "... out of
    /// bounds"): kind `out_of_bounds` either way (`dump_records.error_kind`).
    fn eof(&self, need: usize) -> ReadError {
        self.fail(
            ReadErrorKind::OutOfBounds,
            ReadDetail::Eof {
                need: u64::try_from(need).unwrap_or(u64::MAX),
            },
        )
    }

    /// R2: `ptr + n > end` fails. The end of the `n` bytes. A check of `n =
    /// 0` never fails: every one follows a 4-byte length that passed its own
    /// check, so the position is within the bytes (an empty run or string
    /// after an alignment past the end fails at its length, as UnityPyBoost's
    /// `_read_length` and then `ptr + n > end` do, `TypeTreeHelper.cpp:269`,
    /// `:296`; review round 16).
    fn bound(&self, n: usize) -> Result<usize, ReadError> {
        match self.pos.checked_add(n) {
            Some(end) if end <= self.bytes.len() => Ok(end),
            _ => Err(self.eof(n)),
        }
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        let end = self.bound(N)?;
        let b = self
            .bytes
            .get(self.pos..end)
            .and_then(|s| <[u8; N]>::try_from(s).ok())
            .ok_or_else(|| self.eof(N))?;
        self.pos = end;
        Ok(b)
    }

    /// R3: `align4` from the object's start, no bounds check
    /// (UnityPy TypeTreeHelper.cpp:67-72).
    fn align4(&mut self) {
        self.pos = self.pos.checked_add(3).map_or(usize::MAX & !3, |p| p & !3);
    }

    /// `_read_length` (UnityPy TypeTreeHelper.cpp:266-286): bounds, then a
    /// negative length fails before the cursor moves.
    fn length(&mut self) -> Result<usize, ReadError> {
        let end = self.bound(4)?;
        let b = self
            .bytes
            .get(self.pos..end)
            .and_then(|s| <[u8; 4]>::try_from(s).ok())
            .ok_or_else(|| self.eof(4))?;
        let n = num!(self.big, i32, b);
        let Ok(len) = usize::try_from(n) else {
            return Err(self.fail(
                ReadErrorKind::NegativeLength,
                ReadDetail::NegativeLength(i64::from(n)),
            ));
        };
        self.pos = end;
        Ok(len)
    }

    /// R4, R5: one scalar (`read_u8`, `read_s8`, `read_bool`, `read_num`,
    /// UnityPy TypeTreeHelper.cpp:74-204).
    fn prim(&mut self, kind: Kind) -> Result<Value, ReadError> {
        let big = self.big;
        Ok(match kind {
            Kind::U8 => int(i64::from(u8::from_le_bytes(self.fixed()?))),
            Kind::S8 => int(i64::from(i8::from_le_bytes(self.fixed()?))),
            // UnityPy TypeTreeHelper.cpp:81: any non-zero byte is true.
            Kind::Bool => Value::Bool(u8::from_le_bytes(self.fixed()?) != 0),
            Kind::U16 => int(i64::from(num!(big, u16, self.fixed()?))),
            Kind::S16 => int(i64::from(num!(big, i16, self.fixed()?))),
            Kind::U32 => int(i64::from(num!(big, u32, self.fixed()?))),
            Kind::S32 => int(i64::from(num!(big, i32, self.fixed()?))),
            Kind::U64 => uint(num!(big, u64, self.fixed()?)),
            Kind::S64 => int(num!(big, i64, self.fixed()?)),
            Kind::F32 => Value::F32(f32_bits(num!(big, u32, self.fixed()?), self.floats)),
            Kind::F64 => Value::F64(num!(big, u64, self.fixed()?)),
            _ => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::Python("not a primitive")));
            }
        })
    }

    fn value(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let at = self.at();
        self.work.enter(at)?;
        let out = self.value_inner(t, lx, node);
        self.work.leave();
        out
    }

    /// A class's or a `ReferencedObject`'s child, read by `read`: under a
    /// name a sibling repeats, every value it builds counts against
    /// `max_values_per_object` (`Work`'s docs).
    #[inline]
    fn child(
        &mut self,
        repeated: bool,
        read: impl FnOnce(&mut Self) -> Result<Out, ReadError>,
    ) -> Result<Out, ReadError> {
        if !repeated {
            return read(self);
        }
        self.work.enter_repeated();
        let out = read(self);
        self.work.leave_repeated();
        out
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883).
    fn value_inner(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let me = t.info(node).copied().ok_or_else(|| {
            self.fail(
                ReadErrorKind::Other,
                ReadDetail::Python("node outside its tree"),
            )
        })?;
        self.work.kept(self.at())?;
        // R15: a flagged node aligns after its value.
        let mut align = me.align;
        let out = match me.shape {
            Shape::Prim => Out::Plain(self.prim(me.kind)?),
            // R6 (UnityPy TypeTreeHelper.cpp:288-305): length, bounds, the
            // raw bytes, then always aligned.
            Shape::Str => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let s = RawStr::from_shared(self.data.slice(self.pos..end));
                self.pos = end;
                self.align4();
                Out::Plain(Value::Str(s))
            }
            // R7 (UnityPy TypeTreeHelper.cpp:307-323): aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                Out::Plain(Value::Bytes(b))
            }
            // R8 (UnityPy TypeTreeHelper.cpp:325-351).
            Shape::Pair => {
                let (a, b) = two(t, node);
                let first = self.value(t, lx, a)?;
                let second = self.value(t, lx, b)?;
                self.tuple2(first, second)?
            }
            Shape::BadPair => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::PairChildren));
            }
            Shape::RefObject => self.ref_object(t, lx, node)?,
            // R9 (UnityPy TypeTreeHelper.cpp:812-861).
            Shape::Vector {
                elem,
                run,
                array_align,
            } => {
                align |= array_align;
                let count = self.length()?;
                let ei = t.info(elem).copied().unwrap_or(me);
                if run {
                    self.run(t, lx, elem, ei, count)?
                } else {
                    self.list(t, lx, elem, ei, count)?
                }
            }
            Shape::BadVector => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::ArrayChildren));
            }
            Shape::Class => self.class(t, lx, node, me)?,
        };
        if align {
            self.align4();
        }
        Ok(out)
    }

    fn tuple2(&mut self, first: Out, second: Out) -> Result<Out, ReadError> {
        let at = self.at();
        let mut items = self.builder.vec();
        items.reserve_exact(2).map_err(|e| charge_error(at, e))?;
        first.push_to(&mut items).map_err(|e| charge_error(at, e))?;
        second
            .push_to(&mut items)
            .map_err(|e| charge_error(at, e))?;
        let built = self.builder.tuple(items).map_err(|e| charge_error(at, e))?;
        Ok(Out::Built(built))
    }

    /// R11 (UnityPy TypeTreeHelper.cpp:846-855): elements read one by one,
    /// each aligning itself.
    fn list(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        elem: NodeId,
        ei: NodeInfo,
        count: usize,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let first = self.work.first_room(count, remaining, ei.min_size);
        let mut items = self.builder.vec();
        items
            .reserve_exact(first)
            .map_err(|e| charge_error(at, e))?;
        for i in 0..count {
            grow_step(&mut items, count).map_err(|e| charge_error(self.at(), e))?;
            let (before, mark) = (self.pos, self.work.discards());
            let v = self.value(t, lx, elem)?;
            if self.pos <= before {
                self.work.zero_size(mark, self.at())?;
            }
            v.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
            self.work.filled(i, first);
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// R10: `read_typetree_value_array` (UnityPy TypeTreeHelper.cpp:886-940):
    /// a primitive or `pair` run, aligned once after when the element is
    /// flagged.
    fn run(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        elem: NodeId,
        ei: NodeInfo,
        count: usize,
    ) -> Result<Out, ReadError> {
        let big = self.big;
        let floats = self.floats;
        let out = match ei.kind {
            Kind::U8 => {
                let end = self.bound(count)?;
                self.work.run(as_u64(count), self.at())?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                Out::Plain(Value::Prims(PrimArray::U8(b)))
            }
            Kind::Bool => self.nums::<1, _>(count, |b| u8::from_le_bytes(b) != 0, PrimRun::Bool)?,
            Kind::S8 => self.nums::<1, _>(count, i8::from_le_bytes, PrimRun::I8)?,
            Kind::U16 => self.nums::<2, _>(count, |b| num!(big, u16, b), PrimRun::U16)?,
            Kind::S16 => self.nums::<2, _>(count, |b| num!(big, i16, b), PrimRun::I16)?,
            Kind::U32 => self.nums::<4, _>(count, |b| num!(big, u32, b), PrimRun::U32)?,
            Kind::S32 => self.nums::<4, _>(count, |b| num!(big, i32, b), PrimRun::I32)?,
            Kind::U64 => self.nums::<8, _>(count, |b| num!(big, u64, b), PrimRun::U64)?,
            Kind::S64 => self.nums::<8, _>(count, |b| num!(big, i64, b), PrimRun::I64)?,
            Kind::F32 => {
                self.nums::<4, _>(count, |b| f32_bits(num!(big, u32, b), floats), PrimRun::F32)?
            }
            Kind::F64 => self.nums::<8, _>(count, |b| num!(big, u64, b), PrimRun::F64)?,
            _ => self.pairs(t, lx, elem, ei, count)?,
        };
        if ei.align {
            self.align4();
        }
        Ok(out)
    }

    /// `read_num_array` and friends (UnityPy TypeTreeHelper.cpp:86-264):
    /// the whole run's bounds first, then every element.
    #[inline(never)]
    fn nums<const N: usize, T>(
        &mut self,
        count: usize,
        conv: impl Fn([u8; N]) -> T,
        wrap: fn(MeteredVec<T>) -> PrimRun,
    ) -> Result<Out, ReadError> {
        let total = count.checked_mul(N).ok_or_else(|| self.eof(usize::MAX))?;
        let end = self.bound(total)?;
        let at = self.at();
        self.work.run(as_u64(total), at)?;
        let mut items: MeteredVec<T> = self.builder.vec();
        // The bytes are there: the run's memory is that of its bytes.
        items
            .reserve_exact(count)
            .map_err(|e| charge_error(at, e))?;
        let (chunks, _) = self
            .bytes
            .get(self.pos..end)
            .unwrap_or_default()
            .as_chunks::<N>();
        for b in chunks {
            items.push(conv(*b)).map_err(|e| charge_error(at, e))?;
        }
        self.pos = end;
        let built = self
            .builder
            .prims(wrap(items))
            .map_err(|e| charge_error(at, e))?;
        Ok(Out::Built(built))
    }

    /// `read_pair_array` (UnityPy TypeTreeHelper.cpp:353-392): the arity is
    /// checked once, before the first element (R10), so even an empty run of
    /// malformed pairs fails.
    fn pairs(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        elem: NodeId,
        ei: NodeInfo,
        count: usize,
    ) -> Result<Out, ReadError> {
        if ei.shape != Shape::Pair {
            return Err(self.fail(ReadErrorKind::Other, ReadDetail::PairChildren));
        }
        let (a, b) = two(t, elem);
        let at = self.at();
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let reserved = self.work.first_room(count, remaining, ei.min_size);
        let mut items = self.builder.vec();
        items
            .reserve_exact(reserved)
            .map_err(|e| charge_error(at, e))?;
        for i in 0..count {
            grow_step(&mut items, count).map_err(|e| charge_error(self.at(), e))?;
            self.work.kept(self.at())?;
            let (before, mark) = (self.pos, self.work.discards());
            let first = self.value(t, lx, a)?;
            let second = self.value(t, lx, b)?;
            if self.pos <= before {
                self.work.zero_size(mark, self.at())?;
            }
            let pair = self.tuple2(first, second)?;
            pair.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
            self.work.filled(i, reserved);
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// R12, R13: `read_class` (UnityPy TypeTreeHelper.cpp:394-445). Its
    /// dict grows as its children are read ([`Dict`]): no pass over them
    /// first (review round 7: a class whose children repeat a name or hold
    /// a registry passed over all of them to size its dict, so a read that
    /// failed at its first child cost what all of them did).
    fn class(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        me: NodeInfo,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        let mut dict = Dict::new(&self.builder, kids.len(), me.dup_names, at)?;
        let mut changed = false;
        for c in kids {
            visit();
            let ci = t.info(*c).copied().unwrap_or(me);
            if ci.kind == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    // A skipped child counts as a value (plan 3.10, "Bounds").
                    self.work.skipped(self.at())?;
                    continue;
                }
                changed = true;
                self.has_registry = true;
            }
            let v = self.child(ci.repeated, |r| r.value(t, lx, *c))?;
            let at = self.at();
            dict.set(&mut self.work, ci.key, ci.slot, v, at)?;
        }
        if changed {
            self.has_registry = false;
        }
        dict.finish(&self.builder, lx, self.at())
    }

    /// R14: a `ReferencedObject` (UnityPy TypeTreeHelper.cpp:751-811). Its
    /// dict grows as its children are read and keeps each name's position
    /// ([`Dict`]), and its `type` is the entry of the first child named
    /// `type` read so far: no pass over its children first (review round 7).
    fn ref_object(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        let mut dict = Dict::new(&self.builder, kids.len(), true, at)?;
        // The slot of the children named `type`, once one is visited: a
        // data child before it finds no `type`, as UnityPyBoost's lookup in
        // the dict read so far finds none.
        let mut type_slot = None;
        // The current `type`'s resolution, made at the first data child that
        // asks and kept until a child sets `type` again: a node of many data
        // children resolves each `type` value once (claude X, review round 2).
        let mut resolved: Option<Option<RefNode>> = None;
        for c in kids {
            visit();
            let Some(ci) = t.info(*c).copied() else {
                continue;
            };
            if type_slot.is_none() && t.list().name(*c) == b"type" {
                type_slot = Some(ci.slot);
            }
            let v = if ci.kind == Kind::ReferencedObjectData {
                let node = if let Some(r) = &resolved {
                    r.clone()
                } else {
                    let ty = type_slot
                        .and_then(|s| dict.position(s))
                        .and_then(|p| dict.entries.as_slice().get(p))
                        .map(|(_, v)| v);
                    resolved.insert(self.ref_node(ty)?).clone()
                };
                match node {
                    None | Some(RefNode::NoTree) => {
                        // A skipped child counts as a value (plan 3.10, "Bounds").
                        self.work.skipped(self.at())?;
                        continue;
                    }
                    Some(RefNode::Tree(rt)) => {
                        let at = self.at();
                        let rlx = self.trees.index(&mut self.builder, &rt, at)?;
                        self.child(ci.repeated, |r| r.value(&rt, rlx, rt.root()))?
                    }
                }
            } else {
                self.child(ci.repeated, |r| r.value(t, lx, *c))?
            };
            let at = self.at();
            dict.set(&mut self.work, ci.key, ci.slot, v, at)?;
            if Some(ci.slot) == type_slot {
                resolved = None;
            }
        }
        dict.finish(&self.builder, lx, self.at())
    }

    /// `get_ref_type_node` (UnityPy TypeTreeHelper.cpp:585-681) on the dict
    /// read so far, whose `type` entry is `ty`: `None` for an empty class
    /// (the data is skipped), else the matched ref type's node. Every
    /// failure is `ref_type`: the caller replaces the message with "Failed
    /// to get ref type node" (`:763`).
    fn ref_node(&mut self, ty: Option<&Value>) -> Result<Option<RefNode>, ReadError> {
        let ref_type = |d: ReadDetail| self.fail(ReadErrorKind::RefType, d);
        if let RefTypes::NoFile | RefTypes::Absent = self.ref_types {
            // `:587-591`, `:592-598`.
            return Err(ref_type(ReadDetail::NoRefTypes));
        }
        // `:600-606`: `type` read yet; `:611-635`: a dict with all three
        // keys (any other object the reader makes has no such attributes).
        let Some(Value::Dict(m)) = ty else {
            return Err(ref_type(ReadDetail::RefTypeNotFound));
        };
        let at = self.at();
        let mut field = |name: &[u8]| {
            self.trees
                .field(m, name, &mut self.work.open)
                .map_err(|l| limit_refused(at, l))
        };
        let (Some(cls), Some(ns), Some(asm)) = (field(b"class")?, field(b"ns")?, field(b"asm")?)
        else {
            return Err(self.fail(ReadErrorKind::RefType, ReadDetail::RefTypeNotFound));
        };
        // `:637-644`: `PyUnicode_GET_LENGTH(cls) == 0` skips the data.
        if py_size_field_is_zero(cls) {
            return Ok(None);
        }
        // `:646-673`: the first ref type whose three names compare equal.
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

/// A `pair`'s two children (its shape checked them).
fn two(t: &Tree, node: NodeId) -> (NodeId, NodeId) {
    match t.list().children(node) {
        [a, b, ..] => (*a, *b),
        [a] => (*a, *a),
        [] => (node, node),
    }
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
