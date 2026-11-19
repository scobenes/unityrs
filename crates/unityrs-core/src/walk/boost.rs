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

use super::dict::{FindError, RefIndex, RefNode, Trees, ref_tree};
use super::{
    FloatMode, Out, ReadCtx, ReadDetail, ReadError, ReadErrorKind, RefTypes, Work, charge_error,
    f32_bits, int, limit_refused, py_size_field_is_zero, uint,
};
use crate::limits::MeteredVec;
use crate::stream::Endian;
use crate::typetree::{Kind, NodeId, Tree};
use crate::value::{Parsed, PrimRun, Value, ValueBuilder};

/// Reads one value from exactly `data` (the object's bytes). `TypelessData`
/// and `vector<UInt8>` values are zero-copy slices of `data`.
/// Its work is the open's, counted on `ctx.meter` whatever the outcome
/// (the module docs of [`super`], "Work").
pub fn read(data: &Bytes, tree: &Arc<Tree>, ctx: &ReadCtx<'_>) -> Result<Parsed, ReadError> {
    read_with_fields(data, tree, ctx, None)
}

/// Read ordered class fields for a binding that must construct completed classes
/// before surfacing a later reader error. Limits and allocation refusals remain
/// immediate; the returned progress retains ordinary reader failures.
pub fn read_object_fields(
    data: &Bytes,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
) -> Result<(Parsed, super::FieldProgress), ReadError> {
    let mut progress = super::FieldProgress::default();
    let parsed = read_with_fields(data, tree, ctx, Some(&mut progress))?;
    Ok((parsed, progress))
}

pub(crate) fn read_with_fields(
    data: &Bytes,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    progress: Option<&mut super::FieldProgress>,
) -> Result<Parsed, ReadError> {
    read_resolved(data, tree, ctx, progress, None)
}

/// Caller-supplied reference lookup, invoked only for a selected data field.
/// The resolver returns the selected tree, or `None` for a disabled tree.
type ReferenceResolver<'a> =
    dyn FnMut(&Value, &Value, &Value, u64) -> Result<Option<Arc<Tree>>, ReadError> + 'a;

/// Read with a lazy reference resolver, retaining ordinary object-field progress.
pub fn read_resolved(
    data: &Bytes,
    tree: &Arc<Tree>,
    ctx: &ReadCtx<'_>,
    progress: Option<&mut super::FieldProgress>,
    resolver: Option<&mut ReferenceResolver<'_>>,
) -> Result<Parsed, ReadError> {
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
        method: "read_length",
        progress,
        resolver,
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
struct Reader<'a, 'c, 'p, 'r, 's> {
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
    method: &'static str,
    progress: Option<&'p mut super::FieldProgress>,
    resolver: Option<&'r mut ReferenceResolver<'s>>,
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

impl Reader<'_, '_, '_, '_, '_> {
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
    fn eof(&self, need: usize, exception: &'static str) -> ReadError {
        self.fail(
            ReadErrorKind::OutOfBounds,
            ReadDetail::BoostEof {
                method: self.method,
                need: u64::try_from(need).unwrap_or(u64::MAX),
                exception,
            },
        )
    }

    /// R2: `ptr + n > end` fails. The end of the `n` bytes. A check of `n =
    /// 0` never fails: every one follows a 4-byte length that passed its own
    /// check, so the position is within the bytes (an empty run or string
    /// after an alignment past the end fails at its length, as UnityPyBoost's
    /// `_read_length` and then `ptr + n > end` do, `TypeTreeHelper.cpp:269`,
    /// `:296`; review round 16).
    fn bound(&self, n: usize, exception: &'static str) -> Result<usize, ReadError> {
        match self.pos.checked_add(n) {
            Some(end) if end <= self.bytes.len() => Ok(end),
            _ => Err(self.eof(n, exception)),
        }
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        let exception = if N == 1 { "ValueError" } else { "EOFError" };
        let end = self.bound(N, exception)?;
        let b = self
            .bytes
            .get(self.pos..end)
            .and_then(|s| <[u8; N]>::try_from(s).ok())
            .ok_or_else(|| self.eof(N, exception))?;
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
        self.method = "read_length";
        let end = self.bound(4, "EOFError")?;
        let b = self
            .bytes
            .get(self.pos..end)
            .and_then(|s| <[u8; 4]>::try_from(s).ok())
            .ok_or_else(|| self.eof(4, "EOFError"))?;
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
        self.method = read_method(kind, false);
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

    /// `read_num_array` and friends (UnityPy TypeTreeHelper.cpp:86-264):
    /// the whole run's bounds first, then every element.
    #[inline(never)]
    fn nums<const N: usize, T>(
        &mut self,
        count: usize,
        conv: impl Fn([u8; N]) -> T,
        wrap: fn(MeteredVec<T>) -> PrimRun,
    ) -> Result<Out, ReadError> {
        let exception = if N == 1 { "ValueError" } else { "EOFError" };
        let total = count
            .checked_mul(N)
            .ok_or_else(|| self.eof(usize::MAX, exception))?;
        let end = self.bound(total, exception)?;
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

    /// `get_ref_type_node` (UnityPy TypeTreeHelper.cpp:585-681) on the dict
    /// read so far, whose `type` entry is `ty`: `None` for an empty class
    /// (the data is skipped), else the matched ref type's node. Every
    /// failure is `ref_type`: the caller replaces the message with "Failed
    /// to get ref type node" (`:763`).
    fn ref_node(
        &mut self,
        ty: Option<&Value>,
        object_type: bool,
    ) -> Result<Option<RefNode>, ReadError> {
        let ref_type = |d: ReadDetail| self.fail(ReadErrorKind::RefType, d);
        if let RefTypes::NoFile | RefTypes::Absent = self.ref_types {
            // `:587-591`, `:592-598`.
            return Err(ref_type(ReadDetail::NoRefTypes {
                reader: crate::value::ReadBy::Boost,
                absent: matches!(self.ref_types, RefTypes::Absent),
            }));
        }
        // `:600-606`: `type` read yet; `:611-635`: a dict with all three
        // keys (any other object the reader makes has no such attributes).
        let Some(Value::Dict(m)) = ty else {
            return Err(ref_type(ReadDetail::RefTypeNotFound));
        };
        let at = self.at();
        let mut field = |name: &[u8]| {
            self.trees
                .field(m, name, &mut self.work.open, object_type)
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
        if let Some(resolve) = &mut self.resolver {
            return resolve(cls, ns, asm, at)
                .map(|tree| Some(tree.map_or(RefNode::NoTree, RefNode::Tree)));
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

// TypeTreeHelper.cpp's typeid(T).name() uses the platform C++ ABI.
fn read_method(kind: Kind, array: bool) -> &'static str {
    macro_rules! name {
        ($suffix:literal) => {
            if array {
                concat!("read_", $suffix, "_array")
            } else {
                concat!("read_", $suffix)
            }
        };
    }
    match kind {
        Kind::Bool => name!("bool"),
        Kind::U8 => name!("u8"),
        Kind::S8 => name!("s8"),
        Kind::U16 => name!("t"),
        Kind::S16 => name!("s"),
        Kind::U32 => name!("j"),
        Kind::S32 => name!("i"),
        Kind::U64 if cfg!(target_os = "macos") => name!("y"),
        Kind::S64 if cfg!(target_os = "macos") => name!("x"),
        Kind::U64 => name!("m"),
        Kind::S64 => name!("l"),
        Kind::F32 => name!("f"),
        Kind::F64 => name!("d"),
        _ => "read_length",
    }
}

// After `num!`, which its steps use.
#[path = "boost_states.rs"]
mod states;
#[path = "boost_steps.rs"]
mod steps;

#[cfg(test)]
pub(crate) use steps::OPEN_SIZE;
