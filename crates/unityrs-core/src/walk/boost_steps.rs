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
use crate::limits::{ChargeError, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::{Kind, NodeId, NodeInfo, Shape, Tree};
use crate::value::{ListIx, PrimArray, PrimRun, Value, ValueBuilder};
use crate::walk::f32_bits;

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

/// Open values kept on the native stack before any is kept on the heap.
/// The recursive reader this one replaces charged nothing for a level;
/// to this depth neither does this one, so a read this deep meets its
/// memory bound where that reader did, with the same error at the same
/// offset (`tests/walk_golden.rs`). A level deeper is charged as any of
/// the read's heap is.
const INLINE_OPENS: usize = 32;

/// The read's open values, innermost last: the first [`INLINE_OPENS`] in
/// place, any deeper on a charged heap stack (`spill`), made and grown
/// only when a value opens past them. A child opens straight into the slot
/// past the innermost ([`Self::split`]), and is on the stack once
/// [`Self::commit`] counts it.
struct Opens {
    inline: [Option<Open>; INLINE_OPENS],
    len: usize,
    spill: Option<MeteredVec<Open>>,
    /// The slot past the innermost once the inline ones are full: its
    /// value moves to `spill` when committed.
    next: Option<Open>,
}

impl Opens {
    fn new() -> Self {
        Self {
            inline: [const { None }; INLINE_OPENS],
            len: 0,
            spill: None,
            next: None,
        }
    }

    /// The innermost open value, stepped where it lies, and the slot a
    /// child it opens goes to.
    fn split(&mut self) -> (Option<&mut Open>, &mut Option<Open>) {
        let Self {
            inline,
            len,
            spill,
            next,
        } = self;
        let (below, above) = inline.split_at_mut((*len).min(INLINE_OPENS));
        let top = match len.checked_sub(INLINE_OPENS) {
            None | Some(0) => below.last_mut().and_then(Option::as_mut),
            Some(_) => spill.as_mut().and_then(|s| s.as_mut_slice().last_mut()),
        };
        (top, above.first_mut().unwrap_or(next))
    }

    /// Counts the child [`Self::split`]'s slot holds as the innermost:
    /// one past the inline ones moves to `spill`, charged on `builder`'s
    /// meter.
    fn commit(&mut self, builder: &ValueBuilder) -> Result<(), ChargeError> {
        if self.len >= INLINE_OPENS {
            let Some(open) = self.next.take() else {
                return Ok(());
            };
            self.spill.get_or_insert_with(|| builder.vec()).push(open)?;
        }
        self.len += 1;
        Ok(())
    }

    /// Drops the innermost open value in place.
    fn drop_top(&mut self) {
        let Some(at) = self.len.checked_sub(1) else {
            return;
        };
        if let Some(slot) = self.inline.get_mut(at) {
            *slot = None;
        } else if let Some(spill) = &mut self.spill {
            drop(spill.pop());
        }
        self.len = at;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Reader<'_, '_, '_, '_, '_> {
    /// `read_typetree_value` of `node` and all it holds. The innermost open
    /// value is stepped where it lies on the stack, a child read whole is
    /// handed straight back to it, and a child that opens is made in its
    /// place on the stack.
    pub(super) fn value(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
    ) -> Result<Out, ReadError> {
        let mut opens = Opens::new();
        let (_, slot) = opens.split();
        if let Some(out) = self.head(t, lx, node, false, slot)? {
            return Ok(out);
        }
        let mut input = None;
        'read: loop {
            opens
                .commit(&self.builder)
                .map_err(|e| charge_error(self.at(), e))?;
            loop {
                let (Some(open), slot) = opens.split() else {
                    return Err(
                        self.fail(ReadErrorKind::Other, ReadDetail::Python("no value read"))
                    );
                };
                match self.step(&open.tree, open.lx, &mut open.state, &mut input, slot) {
                    Step::Open => break,
                    Step::Done(out) => {
                        input = Some(out);
                        self.close(open.align, open.repeated, &mut input);
                        opens.drop_top();
                        if opens.is_empty() {
                            break 'read;
                        }
                    }
                }
            }
        }
        input.unwrap_or_else(|| {
            Err(self.fail(ReadErrorKind::Other, ReadDetail::Python("no value read")))
        })
    }

    /// A child's read up to its children (`child`, then `value`'s entry):
    /// its value when read whole, else `None` with the value in `opened`.
    pub(super) fn head(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        repeated: bool,
        opened: &mut Option<Open>,
    ) -> Result<Option<Out>, ReadError> {
        if !repeated {
            return self.enter(t, lx, node, false, opened);
        }
        self.work.enter_repeated();
        let read = self.enter(t, lx, node, true, opened);
        if !matches!(read, Ok(None)) {
            self.work.leave_repeated();
        }
        read
    }

    /// [`Self::head`] of a node without children: its value.
    pub(super) fn leaf(
        &mut self,
        t: &Tree,
        node: NodeId,
        repeated: bool,
    ) -> Result<Out, ReadError> {
        if repeated {
            self.work.enter_repeated();
        }
        let read = self.enter_leaf(t, node);
        if repeated {
            self.work.leave_repeated();
        }
        read
    }

    /// [`Self::enter`] of a node without children.
    #[inline]
    fn enter_leaf(&mut self, t: &Tree, node: NodeId) -> Result<Out, ReadError> {
        if self.progress.as_ref().is_some_and(|p| p.error.is_some()) {
            return Ok(Out::Plain(Value::Int(0)));
        }
        let at = self.at();
        self.work.enter(at)?;
        self.open_leaf(t, node)
            .or_else(|error| self.leave(Err(error)))
    }

    /// [`Self::open`] of a node without children.
    #[allow(
        clippy::inline_always,
        reason = "measured: a per-value call of the reader"
    )]
    #[inline(always)]
    fn open_leaf(&mut self, t: &Tree, node: NodeId) -> Result<Out, ReadError> {
        let me = t.info(node).copied().ok_or_else(|| {
            self.fail(
                ReadErrorKind::Other,
                ReadDetail::Python("node outside its tree"),
            )
        })?;
        self.work.kept(self.at())?;
        let v = self.scalar(me)?;
        Ok(self.read(v, me.align))
    }

    /// Whether a value opened now is held at a level the stack keeps in
    /// place ([`INLINE_OPENS`]): one past them is charged.
    fn in_place(&self) -> bool {
        usize::try_from(self.work.depth).is_ok_and(|d| d <= INLINE_OPENS)
    }

    /// The end of a class or a list whose children are all read whole,
    /// read where it opens (`step`, run with no stack slot): its value
    /// aligned and its level left, as [`Self::close`] would on the stack,
    /// which would hold it in place ([`Self::in_place`]), so nothing is
    /// charged either way. None of its children opens, so the read takes
    /// no deeper a native stack.
    fn flat(&mut self, step: Step, align: bool) -> Result<Out, ReadError> {
        match step {
            Step::Done(out) => out.map(|out| self.read(out, align)),
            Step::Open => Err(self.fail(ReadErrorKind::Other, ReadDetail::Python("not flat"))),
        }
    }

    /// `value`'s entry: a recorded error reads nothing more; the level,
    /// then `read_typetree_value`.
    #[inline]
    fn enter(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        repeated: bool,
        opened: &mut Option<Open>,
    ) -> Result<Option<Out>, ReadError> {
        if self.progress.as_ref().is_some_and(|p| p.error.is_some()) {
            return Ok(Some(Out::Plain(Value::Int(0))));
        }
        let at = self.at();
        self.work.enter(at)?;
        // A value read whole has left its level ([`Self::read`]).
        self.open(t, lx, node, repeated, opened)
            .or_else(|error| self.leave(Err(error)).map(Some))
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

    /// An open value's end, on its result where it lies: aligned when
    /// read, then `value`'s and `child`'s exits ([`Self::leave`]).
    fn close(&mut self, align: bool, repeated: bool, out: &mut Option<Result<Out, ReadError>>) {
        let read = matches!(out, Some(Ok(_)));
        if read && align {
            self.align4();
        }
        self.work.leave();
        if !read
            && let Some(progress) = &mut self.progress
            && let Some(Err(error)) = out.take()
        {
            *out = Some(progress.recover(error));
        }
        if repeated {
            self.work.leave_repeated();
        }
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883) up to the
    /// node's children: a value without any is read whole (`Some`), one
    /// with children is left in `opened`.
    fn open(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        node: NodeId,
        repeated: bool,
        opened: &mut Option<Open>,
    ) -> Result<Option<Out>, ReadError> {
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
            Shape::Prim | Shape::Str | Shape::Bytes => {
                let v = self.scalar(me)?;
                return Ok(Some(self.read(v, align)));
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
                match self.vector(t, lx, me, elem, run, &mut align, opened)? {
                    Ok(state) => state,
                    Err(read) => return Ok(Some(read)),
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
                let mut fields = Fields::new(node, dict);
                // Its children's shapes, found at the compile ([`NodeInfo::leaves`]).
                if me.leaves && self.in_place() {
                    let step = self.class::<true>(&mut fields, t, lx, &mut None, opened);
                    return self.flat(step, align).map(Some);
                }
                State::Class(fields)
            }
        };
        *opened = Some(Open {
            tree: t.clone(),
            lx,
            align,
            repeated,
            state,
        });
        Ok(None)
    }

    /// R9's vector after its length: a run, or a list of elements read
    /// whole ([`Self::flat`]), read whole (`Err`: the value, aligned and
    /// its level left), or the state of its elements.
    #[allow(
        clippy::too_many_arguments,
        reason = "the vector's node, then its read"
    )]
    fn vector(
        &mut self,
        t: &Arc<Tree>,
        lx: ListIx,
        me: NodeInfo,
        elem: NodeId,
        run: bool,
        align: &mut bool,
        opened: &mut Option<Open>,
    ) -> Result<Result<State, Out>, ReadError> {
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
        let mut elements = Elements::new(elem, count, first, items);
        if self.in_place() && scalar(&ei) {
            let step = self.elements::<true>(&mut elements, t, lx, &mut None, opened);
            return self.flat(step, *align).map(Err);
        }
        Ok(Ok(State::List(elements)))
    }

    /// A value read whole, aligned when flagged; its level left (`leave`
    /// of a value read).
    fn read(&mut self, out: Out, align: bool) -> Out {
        if align {
            self.align4();
        }
        self.work.leave();
        out
    }

    /// A node without children (R4-R7), before its alignment.
    #[allow(
        clippy::inline_always,
        reason = "measured: a per-value call of the reader"
    )]
    #[inline(always)]
    fn scalar(&mut self, me: NodeInfo) -> Result<Out, ReadError> {
        Ok(Out::Plain(match me.shape {
            Shape::Prim => self.prim(me.kind)?,
            // R6 (UnityPy TypeTreeHelper.cpp:288-305): length, bounds, the
            // raw bytes, then always aligned.
            Shape::Str => {
                let len = self.length()?;
                self.method = "read_str";
                let end = self.bound(len, "EOFError")?;
                let s = RawStr::from_shared(self.data.slice(self.pos..end));
                self.pos = end;
                self.align4();
                Value::Str(s)
            }
            // R7 (UnityPy TypeTreeHelper.cpp:307-323): aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                self.method = "read_bytes";
                let end = self.bound(len, "ValueError")?;
                let b = self.data.slice(self.pos..end);
                self.pos = end;
                Value::Bytes(b)
            }
            _ => {
                return Err(self.fail(ReadErrorKind::Other, ReadDetail::Python("not a scalar")));
            }
        }))
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

/// Whether a node is read whole, with no children ([`Reader::scalar`]).
fn scalar(ci: &NodeInfo) -> bool {
    matches!(ci.shape, Shape::Prim | Shape::Str | Shape::Bytes)
}
