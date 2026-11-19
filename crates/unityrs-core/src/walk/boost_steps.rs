// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! The reader's nesting without recursion: `read_typetree_value` (UnityPy
//! TypeTreeHelper.cpp:702-883), `read_class` (`:394-445`), the
//! `ReferencedObject` read (`:751-811`), `read_pair_array` (`:353-392`) and
//! the list of elements read one by one (`:846-855`) each keep the state of
//! a value whose children are still being read in an [`Open`] on a heap
//! stack, so a value of any depth takes the same native stack and a read
//! runs on a thread of any stack size. Each open value does what its
//! recursive form did, in the same order: a child's read is the step that
//! asks for it, and its result (an error passed up, or recovered by the
//! field progress, at every level as the recursive calls passed it) the
//! step that takes it.

use std::sync::Arc;

use super::super::dict::Dict;
use super::super::{Out, ReadDetail, ReadError, ReadErrorKind, charge_error};
use super::states::{Elements, Fields, RefFields, Resolved, State, Step};
use super::{Reader, as_u64, read_method, two};
use crate::limits::MeteredVec;
use crate::raw::RawStr;
use crate::typetree::{Kind, NodeId, NodeInfo, Shape, Tree};
use crate::value::{ListIx, PrimArray, PrimRun, Value};
use crate::walk::f32_bits;

/// The bytes of one open value on the read's stack of them.
#[cfg(test)]
pub(crate) const OPEN_SIZE: usize = size_of::<Open>();

/// A value whose children are being read.
pub(super) struct Open {
    tree: Arc<Tree>,
    lx: ListIx,
    /// Aligned after it is read: R15's flag, a vector's array flag, a
    /// `pair` run's element flag (each `align4` once; a second does
    /// nothing).
    align: bool,
    /// Read as a child whose name a sibling repeats
    /// ([`super::super::Work::enter_repeated`]).
    repeated: bool,
    state: State,
}

/// What a value's first look found.
#[allow(
    clippy::large_enum_variant,
    reason = "moved once a value; boxed, it would allocate outside the meter"
)]
enum Head {
    /// Read whole: `value`'s result.
    Read(Result<Out, ReadError>),
    /// A value with children to read.
    Open(Open),
}

#[allow(
    clippy::large_enum_variant,
    reason = "moved once a step; boxed, it would allocate outside the meter"
)]
enum Resumed {
    /// The open, then the child it opened.
    Push(Open, Open),
    Done(Open, Result<Out, ReadError>),
}

impl Reader<'_, '_, '_, '_, '_> {
    /// `read_typetree_value` of `node` and all it holds.
    pub(super) fn value(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
    ) -> Result<Out, ReadError> {
        // The open values, charged as any of the read's heap is (the
        // recursion they replace took native stack).
        let mut opens: MeteredVec<Open> = self.builder.vec();
        let mut input = match self.head(t, lx, node, false) {
            Head::Read(out) => return out,
            Head::Open(open) => {
                opens.push(open).map_err(|e| charge_error(self.at(), e))?;
                None
            }
        };
        while let Some(open) = opens.pop() {
            match self.resume(open, input.take()) {
                Resumed::Push(open, child) => {
                    opens.push(open).map_err(|e| charge_error(self.at(), e))?;
                    opens.push(child).map_err(|e| charge_error(self.at(), e))?;
                }
                Resumed::Done(open, out) => {
                    let out = self.close(&open, out);
                    drop(open);
                    if opens.is_empty() {
                        return out;
                    }
                    input = Some(out);
                }
            }
        }
        Err(self.fail(ReadErrorKind::Other, ReadDetail::Python("no value read")))
    }

    /// Runs `open`'s steps, reading whole children in place, until it opens
    /// a child or is done.
    fn resume(&mut self, mut open: Open, mut input: Option<Result<Out, ReadError>>) -> Resumed {
        loop {
            match self.step(&open.tree, open.lx, &mut open.state, input.take()) {
                Step::Done(out) => return Resumed::Done(open, out),
                Step::Child {
                    tree,
                    lx,
                    node,
                    repeated,
                } => {
                    let t = tree.as_ref().unwrap_or(&open.tree);
                    match self.head(t, lx, node, repeated) {
                        Head::Read(out) => input = Some(out),
                        Head::Open(child) => return Resumed::Push(open, child),
                    }
                }
            }
        }
    }

    /// A child's read up to its children (`child`, then `value`'s entry).
    fn head(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId, repeated: bool) -> Head {
        if repeated {
            self.work.enter_repeated();
        }
        let head = self.enter(t, lx, node, repeated);
        if repeated && matches!(head, Head::Read(_)) {
            self.work.leave_repeated();
        }
        head
    }

    /// `value`'s entry: a recorded error reads nothing more; the level,
    /// then `read_typetree_value`.
    fn enter(&mut self, t: &Arc<Tree>, lx: ListIx, node: NodeId, repeated: bool) -> Head {
        if self.progress.as_ref().is_some_and(|p| p.error.is_some()) {
            return Head::Read(Ok(Out::Plain(Value::Int(0))));
        }
        let at = self.at();
        if let Err(error) = self.work.enter(at) {
            return Head::Read(Err(error));
        }
        match self.open(t, lx, node, repeated) {
            Ok(Head::Read(out)) => Head::Read(self.leave(out)),
            Ok(open) => open,
            Err(error) => Head::Read(self.leave(Err(error))),
        }
    }

    /// `value`'s exit: the level left, and an error recovered when the
    /// fields' progress takes it.
    fn leave(&mut self, out: Result<Out, ReadError>) -> Result<Out, ReadError> {
        self.work.leave();
        match (out, &mut self.progress) {
            (Err(error), Some(progress)) => progress.recover(error),
            (out, _) => out,
        }
    }

    /// An open value's end: aligned when read, then `value`'s and
    /// `child`'s exits.
    fn close(&mut self, open: &Open, out: Result<Out, ReadError>) -> Result<Out, ReadError> {
        if out.is_ok() && open.align {
            self.align4();
        }
        let out = self.leave(out);
        if open.repeated {
            self.work.leave_repeated();
        }
        out
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883) up to the
    /// node's children: a value without any is read whole.
    fn open(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        repeated: bool,
    ) -> Result<Head, ReadError> {
        let me = t.info(node).copied().ok_or_else(|| {
            self.fail(
                ReadErrorKind::Other,
                ReadDetail::Python("node outside its tree"),
            )
        })?;
        self.work.kept(self.at())?;
        // R15: a flagged node aligns after its value.
        let mut align = me.align;
        let state = match me.shape {
            Shape::Prim => {
                let v = self.prim(me.kind)?;
                return Ok(self.read(Out::Plain(v), align));
            }
            // R6 (UnityPy TypeTreeHelper.cpp:288-305): length, bounds, the
            // raw bytes, then always aligned.
            Shape::Str => {
                let len = self.length()?;
                self.method = "read_str";
                let end = self.bound(len, "EOFError")?;
                let s = RawStr::from_shared(self.data.slice(self.pos..end));
                self.pos = end;
                self.align4();
                return Ok(self.read(Out::Plain(Value::Str(s)), align));
            }
            // R7 (UnityPy TypeTreeHelper.cpp:307-323): aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                self.method = "read_bytes";
                let end = self.bound(len, "ValueError")?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                return Ok(self.read(Out::Plain(Value::Bytes(b)), align));
            }
            // R8 (UnityPy TypeTreeHelper.cpp:325-351).
            Shape::Pair => {
                let (a, b) = two(t, node);
                State::Pair { a, b, first: None }
            }
            Shape::BadPair => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::PairChildren));
            }
            Shape::RefObject => {
                let at = self.at();
                let kids = t.list().children(node);
                let mut dict = Dict::new(&self.builder, kids.len(), true, at)?;
                dict.preserve_fields(self.progress.is_some());
                State::Ref(
                    Fields::new(node, dict),
                    RefFields {
                        type_slot: None,
                        object_type: false,
                        resolved: Resolved::Not,
                    },
                )
            }
            // R9 (UnityPy TypeTreeHelper.cpp:812-861).
            Shape::Vector {
                elem,
                run,
                array_align,
            } => {
                align |= array_align;
                match self.vector(t, me, elem, run, &mut align)? {
                    Ok(state) => state,
                    Err(read) => return Ok(read),
                }
            }
            Shape::BadVector => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::ArrayChildren));
            }
            // R12, R13: `read_class` (UnityPy TypeTreeHelper.cpp:394-445).
            // Its dict grows as its children are read ([`Dict`]).
            Shape::Class => {
                let at = self.at();
                let kids = t.list().children(node);
                let dict = Dict::new(
                    &self.builder,
                    kids.len(),
                    me.dup_names && self.progress.is_none(),
                    at,
                )?;
                State::Class(Fields::new(node, dict))
            }
        };
        Ok(Head::Open(Open {
            tree: t.clone(),
            lx,
            align,
            repeated,
            state,
        }))
    }

    /// R9's vector after its length: a run read whole (`Err`, the value
    /// read), or the state of its elements.
    fn vector(
        &mut self,
        t: &Arc<Tree>,
        me: NodeInfo,
        elem: NodeId,
        run: bool,
        align: &mut bool,
    ) -> Result<Result<State, Head>, ReadError> {
        let count = self.length()?;
        let ei = t.info(elem).copied().unwrap_or(me);
        if run {
            // R10: `read_typetree_value_array` (UnityPy
            // TypeTreeHelper.cpp:886-940): a primitive or `pair` run,
            // aligned once after when the element is flagged.
            self.method = read_method(ei.kind, true);
            if let Some(out) = self.prims(ei.kind, count) {
                let out = out?;
                if ei.align {
                    self.align4();
                }
                return Ok(Err(self.read(out, *align)));
            }
            *align |= ei.align;
            return self.pairs(t, elem, ei, count).map(Ok);
        }
        // R11: elements read one by one, each aligning itself.
        let at = self.at();
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let first = self.work.first_room(count, remaining, ei.min_size);
        let mut items = self.builder.vec();
        items
            .reserve_exact(first)
            .map_err(|e| charge_error(at, e))?;
        Ok(Ok(State::List(Elements::new(elem, count, first, items))))
    }

    /// A value read whole, aligned when flagged.
    fn read(&mut self, out: Out, align: bool) -> Head {
        if align {
            self.align4();
        }
        Head::Read(Ok(out))
    }

    /// A primitive run (`read_num_array` and friends), or `None` for a
    /// `pair` run.
    fn prims(&mut self, kind: Kind, count: usize) -> Option<Result<Out, ReadError>> {
        let big = self.big;
        let floats = self.floats;
        Some(match kind {
            Kind::U8 => (|| {
                let end = self.bound(count, "ValueError")?;
                self.work.run(as_u64(count), self.at())?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                Ok(Out::Plain(Value::Prims(PrimArray::U8(b))))
            })(),
            Kind::Bool => self.nums::<1, _>(count, |b| u8::from_le_bytes(b) != 0, PrimRun::Bool),
            Kind::S8 => self.nums::<1, _>(count, i8::from_le_bytes, PrimRun::I8),
            Kind::U16 => self.nums::<2, _>(count, |b| num!(big, u16, b), PrimRun::U16),
            Kind::S16 => self.nums::<2, _>(count, |b| num!(big, i16, b), PrimRun::I16),
            Kind::U32 => self.nums::<4, _>(count, |b| num!(big, u32, b), PrimRun::U32),
            Kind::S32 => self.nums::<4, _>(count, |b| num!(big, i32, b), PrimRun::I32),
            Kind::U64 => self.nums::<8, _>(count, |b| num!(big, u64, b), PrimRun::U64),
            Kind::S64 => self.nums::<8, _>(count, |b| num!(big, i64, b), PrimRun::I64),
            Kind::F32 => {
                self.nums::<4, _>(count, |b| f32_bits(num!(big, u32, b), floats), PrimRun::F32)
            }
            Kind::F64 => self.nums::<8, _>(count, |b| num!(big, u64, b), PrimRun::F64),
            _ => return None,
        })
    }

    /// `read_pair_array` (UnityPy TypeTreeHelper.cpp:353-392) up to its
    /// elements: the arity is checked once, before the first element (R10),
    /// so even an empty run of malformed pairs fails.
    fn pairs(
        &mut self,
        t: &Arc<Tree>,
        elem: NodeId,
        ei: NodeInfo,
        count: usize,
    ) -> Result<State, ReadError> {
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
        Ok(State::Pairs {
            a,
            b,
            elements: Elements::new(elem, count, reserved, items),
            first: None,
        })
    }
}
