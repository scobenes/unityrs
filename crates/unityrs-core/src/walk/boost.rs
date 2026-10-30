// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! UnityPyBoost's reader (`UnityPyBoost/TypeTreeHelper.cpp:67-1056`), for
//! objects with `byte_size > 0`. The rules, each pinned by a test (plan
//! section 3.10, R1-R16): bounds checks `pos + n > end` evaluated even for
//! `n = 0`; `align4` from the object's start with no bounds check; strings
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

use super::dict::{Dict, RefIndex, RefNode, Trees, ref_tree, slot_count, slot_of, visit};
use super::{
    FloatMode, Out, ReadCtx, ReadDetail, ReadError, ReadErrorKind, RefTypes, Work, charge_error,
    f32_bits, first_reserve, grow_step, int, py_size_field_is_zero, uint,
};
use crate::limits::MeteredVec;
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{Kind, NodeId, NodeInfo, Shape, Tree};
use crate::value::{ListIx, Parsed, PrimArray, PrimRun, Value, ValueBuilder};

/// Reads one value from exactly `data` (the object's bytes). `TypelessData`
/// and `vector<UInt8>` values are zero-copy slices of `data`.
pub fn read(data: &Bytes, tree: &Arc<Tree>, ctx: &ReadCtx<'_>) -> Result<Parsed, ReadError> {
    let builder =
        ValueBuilder::new(ctx.meter, tree.list().clone()).map_err(|e| charge_error(0, e))?;
    let trees = Trees::new(&builder, tree)?;
    let scratch = builder.vec();
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
        work: Work::new(ctx.options.limits),
        has_registry: false,
        scratch,
    };
    let value = r.value(tree, 0, tree.root())?;
    let at = r.at();
    let built = value.into_built(&r.builder, at)?;
    // UnityPy TypeTreeHelper.cpp:1033: `bytes_read = reader.ptr - reader.start`.
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
    work: Work,
    /// `config->has_registry` (R13).
    has_registry: bool,
    /// The slot scratch of the dicts being read ([`Dict`]).
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

    /// R2: `ptr + n > end` fails, evaluated for `n = 0` too (after an
    /// alignment past the end). The end of the `n` bytes.
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

    /// `_read_length` (UnityPy TypeTreeHelper.cpp:262-281): bounds, then a
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

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-884).
    fn value_inner(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let me = t.info(node).copied().ok_or_else(|| {
            self.fail(
                ReadErrorKind::Other,
                ReadDetail::Python("node outside its tree"),
            )
        })?;
        self.work.values(1, self.at())?;
        // R15: a flagged node aligns after its value.
        let mut align = me.align;
        let out = match me.shape {
            Shape::Prim => Out::Plain(self.prim(me.kind)?),
            // R6 (UnityPy TypeTreeHelper.cpp:283-300): length, bounds, the
            // raw bytes, then always aligned.
            Shape::Str => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let s = RawStr::from_shared(self.data.slice(self.pos..end));
                self.pos = end;
                self.align4();
                Out::Plain(Value::Str(s))
            }
            // R7 (UnityPy TypeTreeHelper.cpp:303-319): aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                Out::Plain(Value::Bytes(b))
            }
            // R8 (UnityPy TypeTreeHelper.cpp:322-347).
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

    /// R11 (UnityPy TypeTreeHelper.cpp:835-849): elements read one by one,
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
        let max = self.work.limits.max_prealloc;
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let mut items = self.builder.vec();
        items
            .reserve_exact(first_reserve(count, remaining, ei.min_size, max))
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
                self.work.values(as_u64(count), self.at())?;
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

    /// `read_num_array` and friends (UnityPy TypeTreeHelper.cpp:88-260):
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
        self.work.values(as_u64(count), at)?;
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

    /// `read_pair_array` (UnityPy TypeTreeHelper.cpp:350-391): the arity is
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
        let max = self.work.limits.max_prealloc;
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let mut items = self.builder.vec();
        items
            .reserve_exact(first_reserve(count, remaining, ei.min_size, max))
            .map_err(|e| charge_error(at, e))?;
        for _ in 0..count {
            grow_step(&mut items, count, max).map_err(|e| charge_error(self.at(), e))?;
            self.work.values(1, self.at())?;
            let before = self.pos;
            let first = self.value(t, lx, a)?;
            let second = self.value(t, lx, b)?;
            if self.pos <= before {
                self.work.zero_size(self.at())?;
            }
            let pair = self.tuple2(first, second)?;
            pair.push_to(&mut items)
                .map_err(|e| charge_error(self.at(), e))?;
        }
        Ok(Out::Built(self.builder.list(items)))
    }

    /// R12, R13: `read_class` (UnityPy TypeTreeHelper.cpp:394-445).
    fn class(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        me: NodeInfo,
    ) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        // The entries the read will set: the distinct names of the
        // children it will not skip (R13: a registry once one is in scope).
        let mut reg = self.has_registry;
        let mut reads = |c: &&NodeId| {
            let registry = t
                .info(**c)
                .is_some_and(|i| i.kind == Kind::ManagedReferencesRegistry);
            if registry && reg {
                return false;
            }
            reg |= registry;
            true
        };
        let (reserve, slots) = if me.dup_names {
            let width = slot_count(t, node);
            let mut seen = self.builder.vec::<bool>();
            seen.reserve_exact(width).map_err(|e| charge_error(at, e))?;
            for _ in 0..width {
                seen.push(false).map_err(|e| charge_error(at, e))?;
            }
            let mut n = 0;
            for c in kids.iter().filter(|c| reads(c)) {
                let slot = t.info(*c).map_or(0, |i| i.slot as usize);
                if let Some(s) = seen.as_mut_slice().get_mut(slot)
                    && !*s
                {
                    *s = true;
                    n += 1;
                }
            }
            (n, width)
        } else {
            (kids.iter().filter(|c| reads(c)).count(), 0)
        };
        let mut dict = Dict::new(&self.builder, &mut self.scratch, reserve, slots, at)?;
        let mut changed = false;
        for c in kids {
            visit();
            let ci = t.info(*c).copied().unwrap_or(me);
            if ci.kind == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    // A skipped child counts as a value (plan 3.10, "Bounds").
                    self.work.values(1, self.at())?;
                    continue;
                }
                changed = true;
                self.has_registry = true;
            }
            let v = self.value(t, lx, *c)?;
            let at = self.at();
            dict.set(&mut self.scratch, ci.key, ci.slot, v, at)?;
        }
        if changed {
            self.has_registry = false;
        }
        Ok(dict.finish(&self.builder, &mut self.scratch, lx))
    }

    /// R14: a `ReferencedObject` (UnityPy TypeTreeHelper.cpp:751-811).
    fn ref_object(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId) -> Result<Out, ReadError> {
        let at = self.at();
        let kids = t.list().children(node);
        // The data child is read only when it resolves: reserved as it comes.
        let width = slot_count(t, node);
        let mut seen = self.builder.vec::<bool>();
        seen.reserve_exact(width).map_err(|e| charge_error(at, e))?;
        for _ in 0..width {
            seen.push(false).map_err(|e| charge_error(at, e))?;
        }
        let mut reserve = 0;
        for c in kids {
            let Some(ci) = t.info(*c) else { continue };
            if ci.kind == Kind::ReferencedObjectData {
                continue;
            }
            if let Some(s) = seen.as_mut_slice().get_mut(ci.slot as usize)
                && !*s
            {
                *s = true;
                reserve += 1;
            }
        }
        drop(seen);
        let type_slot = slot_of(t, node, b"type");
        let mut dict = Dict::new(&self.builder, &mut self.scratch, reserve, width, at)?;
        // The current `type`'s resolution, made at the first data child that
        // asks and kept until a child sets `type` again: a node of many data
        // children resolves each `type` value once (claude X, review round 2).
        let mut resolved: Option<Option<RefNode>> = None;
        for c in kids {
            visit();
            let Some(ci) = t.info(*c).copied() else {
                continue;
            };
            let v = if ci.kind == Kind::ReferencedObjectData {
                let node = if let Some(r) = &resolved {
                    r.clone()
                } else {
                    let ty = type_slot
                        .and_then(|s| dict.position(&self.scratch, s))
                        .and_then(|p| dict.entries.as_slice().get(p))
                        .map(|(_, v)| v);
                    resolved.insert(self.ref_node(ty)?).clone()
                };
                match node {
                    None | Some(RefNode::NoTree) => {
                        // A skipped child counts as a value (plan 3.10, "Bounds").
                        self.work.values(1, self.at())?;
                        continue;
                    }
                    Some(RefNode::Tree(rt)) => {
                        let at = self.at();
                        let rlx = self.trees.index(&mut self.builder, &rt, at)?;
                        self.value(&rt, rlx, rt.root())?
                    }
                }
            } else {
                self.value(t, lx, *c)?
            };
            let at = self.at();
            dict.set(&mut self.scratch, ci.key, ci.slot, v, at)?;
            if Some(ci.slot) == type_slot {
                resolved = None;
            }
        }
        Ok(dict.finish(&self.builder, &mut self.scratch, lx))
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
        // `:600-606`: `type` read yet; `:611-633`: a dict with all three
        // keys (any other object the reader makes has no such attributes).
        let Some(Value::Dict(m)) = ty else {
            return Err(ref_type(ReadDetail::RefTypeNotFound));
        };
        let (Some(cls), Some(ns), Some(asm)) = (
            self.trees.field(m, b"class"),
            self.trees.field(m, b"ns"),
            self.trees.field(m, b"asm"),
        ) else {
            return Err(ref_type(ReadDetail::RefTypeNotFound));
        };
        // `:635-642`: `PyUnicode_GET_LENGTH(cls) == 0` skips the data.
        if py_size_field_is_zero(cls) {
            return Ok(None);
        }
        // `:644-670`: the first ref type whose three names compare equal.
        let at = self.at();
        let found = self
            .refs
            .find(self.builder.meter(), cls, ns, asm)
            .map_err(|e| charge_error(at, e))?;
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
