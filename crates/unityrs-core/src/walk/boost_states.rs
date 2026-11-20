// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! The open values of UnityPyBoost's reader ([`super::steps`]): each value
//! with children keeps here what its recursive form kept in its frame, and
//! a step does what that form did between two of its children's reads, in
//! the same order.

use std::sync::Arc;

use super::super::dict::{Dict, RefNode, visit};
use super::super::{Out, ReadDetail, ReadError, ReadErrorKind, charge_error, grow_step};
use super::Reader;
use super::steps::Open;
use crate::limits::MeteredVec;
use crate::typetree::{Kind, NodeId, NodeInfo, Tree};
use crate::value::{ListIx, Value};

pub(super) enum State {
    /// R8: its two children, then the tuple.
    Pair {
        a: NodeId,
        b: NodeId,
        first: Option<Out>,
    },
    /// R11: elements read and aligned one by one.
    List(Elements),
    /// R10: a `pair` run.
    Pairs {
        a: NodeId,
        b: NodeId,
        elements: Elements,
        first: Option<Out>,
    },
    /// R12, R13.
    Class(Fields),
    /// R14.
    Ref(Fields, RefFields),
}

/// A list's elements so far.
pub(super) struct Elements {
    pub(super) elem: NodeId,
    pub(super) count: usize,
    /// Its first reservation ([`super::super::Work::first_room`]).
    pub(super) first: usize,
    pub(super) items: MeteredVec<Value>,
    pub(super) i: usize,
    /// Where the element being read began, and the discards then.
    pub(super) before: usize,
    pub(super) mark: u64,
}

impl Elements {
    pub(super) fn new(elem: NodeId, count: usize, first: usize, items: MeteredVec<Value>) -> Self {
        Self {
            elem,
            count,
            first,
            items,
            i: 0,
            before: 0,
            mark: 0,
        }
    }
}

/// A dict's children so far.
pub(super) struct Fields {
    pub(super) node: NodeId,
    pub(super) dict: Option<Dict>,
    pub(super) next: usize,
    /// R13: this class set the registry in scope.
    pub(super) changed: bool,
    /// The child being read.
    pub(super) pending: NodeId,
}

impl Fields {
    pub(super) fn new(node: NodeId, dict: Dict) -> Self {
        Self {
            node,
            dict: Some(dict),
            next: 0,
            changed: false,
            pending: node,
        }
    }
}

/// A `ReferencedObject`'s `type`.
pub(super) struct RefFields {
    /// The slot of the children named `type`, once one is visited: a data
    /// child before it finds no `type`, as UnityPyBoost's lookup in the dict
    /// read so far finds none.
    pub(super) type_slot: Option<u32>,
    pub(super) object_type: bool,
    /// The current `type`'s resolution, made at the first data child that
    /// asks and kept until a child sets `type` again: a node of many data
    /// children resolves each `type` value once (claude X, review round 2).
    pub(super) resolved: Resolved,
}

pub(super) enum Resolved {
    Not,
    To(Option<RefNode>),
}

/// Where an open value's steps stopped.
pub(super) enum Step {
    /// A child opened (the reader's `opened`): its value comes back as the
    /// next step's `input`.
    Open,
    /// Its own read is over: `read_typetree_value`'s result before its
    /// alignment.
    Done(Result<Out, ReadError>),
}

/// `?` for a step: an error ends the open value's read.
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Step::Done(Err(e)),
        }
    };
}

/// A child's read in place: its value when read whole; a child that opens
/// ends the step, and so does an error (the open value's read is over).
macro_rules! read_child {
    ($self:ident, $tree:expr, $lx:expr, $node:expr, $repeated:expr, $opened:ident) => {
        match $self.head($tree, $lx, $node, $repeated, $opened) {
            Ok(Some(out)) => out,
            Ok(None) => return Step::Open,
            Err(e) => return Step::Done(Err(e)),
        }
    };
    // A child of a value whose children are all read whole
    // ([`super::steps`]' `flat`): never opens.
    (flat $flat:ident, $self:ident, $tree:expr, $lx:expr, $node:expr, $repeated:expr, $opened:ident) => {
        if $flat {
            match $self.leaf($tree, $node, $repeated) {
                Ok(out) => out,
                Err(e) => return Step::Done(Err(e)),
            }
        } else {
            read_child!($self, $tree, $lx, $node, $repeated, $opened)
        }
    };
}

impl Reader<'_, '_, '_, '_, '_> {
    fn stopped(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| p.error.is_some())
    }

    /// An open value's steps up to its next child that opens (left in
    /// `opened`) or its end: `input` is the value of the child that opened
    /// last. A child read whole is taken where it is read, as the recursive
    /// form took its children's values.
    pub(super) fn step(
        &mut self,
        tree: &Arc<Tree>,
        lx: ListIx,
        state: &mut State,
        input: &mut Option<Result<Out, ReadError>>,
        opened: &mut Option<Open>,
    ) -> Step {
        match state {
            State::Pair { a, b, first } => {
                let mut v = match input.take() {
                    Some(out) => tri!(out),
                    None => read_child!(self, tree, lx, *a, false, opened),
                };
                loop {
                    match first.take() {
                        None => {
                            *first = Some(v);
                            v = read_child!(self, tree, lx, *b, false, opened);
                        }
                        Some(f) => return Step::Done(self.tuple2(f, v)),
                    }
                }
            }
            State::List(e) => self.elements::<false>(e, tree, lx, input, opened),
            State::Pairs {
                a,
                b,
                elements,
                first,
            } => self.pairs_step((*a, *b), elements, first, tree, lx, input, opened),
            State::Class(f) => self.class::<false>(f, tree, lx, input, opened),
            State::Ref(f, r) => self.reference(f, r, tree, lx, input, opened),
        }
    }

    /// R11 (UnityPy TypeTreeHelper.cpp:846-855).
    #[inline(never)]
    pub(super) fn elements<const FLAT: bool>(
        &mut self,
        e: &mut Elements,
        tree: &Arc<Tree>,
        lx: ListIx,
        input: &mut Option<Result<Out, ReadError>>,
        opened: &mut Option<Open>,
    ) -> Step {
        let mut read = match input.take() {
            Some(out) => Some(tri!(out)),
            None => None,
        };
        loop {
            if let Some(v) = read.take() {
                if self.pos <= e.before {
                    tri!(self.work.zero_size(e.mark, self.at()));
                }
                tri!(
                    v.push_to(&mut e.items)
                        .map_err(|err| charge_error(self.at(), err))
                );
                if self.stopped() {
                    return self.list(&mut e.items);
                }
                self.work.filled(e.i, e.first);
                e.i += 1;
            }
            if e.i >= e.count {
                return self.list(&mut e.items);
            }
            tri!(grow_step(&mut e.items, e.count).map_err(|err| charge_error(self.at(), err)));
            (e.before, e.mark) = (self.pos, self.work.discards());
            read = Some(read_child!(flat FLAT, self, tree, lx, e.elem, false, opened));
        }
    }

    /// `read_pair_array` (UnityPy TypeTreeHelper.cpp:353-392).
    #[allow(
        clippy::too_many_arguments,
        reason = "the open value's own state, then the step's"
    )]
    fn pairs_step(
        &mut self,
        pair: (NodeId, NodeId),
        e: &mut Elements,
        first: &mut Option<Out>,
        tree: &Arc<Tree>,
        lx: ListIx,
        input: &mut Option<Result<Out, ReadError>>,
        opened: &mut Option<Open>,
    ) -> Step {
        let mut read = match input.take() {
            Some(out) => Some(tri!(out)),
            None => None,
        };
        loop {
            if let Some(v) = read.take() {
                let Some(f) = first.take() else {
                    *first = Some(v);
                    read = Some(read_child!(self, tree, lx, pair.1, false, opened));
                    continue;
                };
                if self.pos <= e.before {
                    tri!(self.work.zero_size(e.mark, self.at()));
                }
                let pair = tri!(self.tuple2(f, v));
                tri!(
                    pair.push_to(&mut e.items)
                        .map_err(|err| charge_error(self.at(), err))
                );
                if self.stopped() {
                    return self.list(&mut e.items);
                }
                self.work.filled(e.i, e.first);
                e.i += 1;
            }
            if e.i >= e.count {
                return self.list(&mut e.items);
            }
            tri!(grow_step(&mut e.items, e.count).map_err(|err| charge_error(self.at(), err)));
            tri!(self.work.kept(self.at()));
            (e.before, e.mark) = (self.pos, self.work.discards());
            read = Some(read_child!(self, tree, lx, pair.0, false, opened));
        }
    }

    /// Sets the child that opened last (`f.pending`) in the dict: false
    /// once the fields' progress has stopped.
    fn set(&mut self, f: &mut Fields, tree: &Tree, v: Out) -> Result<bool, ReadError> {
        if f.dict.is_none() {
            return Err(self.twice());
        }
        let c = f.pending;
        let ci = tree.info(c).or_else(|| tree.info(f.node)).copied();
        let ci = ci.ok_or_else(|| self.twice())?;
        self.put(f, c, ci, v)
    }

    /// Sets child `c`, of info `ci`, in the dict.
    #[allow(
        clippy::inline_always,
        reason = "measured: a per-value call of the reader"
    )]
    #[inline(always)]
    fn put(&mut self, f: &mut Fields, c: NodeId, ci: NodeInfo, v: Out) -> Result<bool, ReadError> {
        let Some(d) = f.dict.as_mut() else {
            return Err(self.twice());
        };
        let at = self.at();
        let key = if self.progress.is_some() { c } else { ci.key };
        d.set_inline(&mut self.work, key, ci.slot, v, at)?;
        Ok(!self.stopped())
    }

    fn twice(&self) -> ReadError {
        self.fail(ReadErrorKind::Other, ReadDetail::Python("class read twice"))
    }

    /// R12, R13: `read_class` (UnityPy TypeTreeHelper.cpp:394-445).
    #[inline(never)]
    pub(super) fn class<const FLAT: bool>(
        &mut self,
        f: &mut Fields,
        tree: &Arc<Tree>,
        lx: ListIx,
        input: &mut Option<Result<Out, ReadError>>,
        opened: &mut Option<Open>,
    ) -> Step {
        let mut more = match input.take() {
            Some(out) => {
                let v = tri!(out);
                tri!(self.set(f, tree, v))
            }
            None => true,
        };
        let Some(&me) = tree.info(f.node) else {
            return Step::Done(Err(self.twice()));
        };
        let kids = tree.list().children(f.node);
        while let Some(&c) = kids.get(f.next).filter(|_| more) {
            f.next += 1;
            visit();
            let ci = tree.info(c).copied().unwrap_or(me);
            if ci.kind == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    // A skipped child counts as a value (plan 3.10, "Bounds").
                    tri!(self.work.skipped(self.at()));
                    continue;
                }
                f.changed = true;
                self.has_registry = true;
            }
            f.pending = c;
            let v = read_child!(flat FLAT, self, tree, lx, c, ci.repeated, opened);
            more = tri!(self.put(f, c, ci, v));
        }
        if f.changed {
            self.has_registry = false;
        }
        self.finish(&mut f.dict, lx)
    }

    /// R14: a `ReferencedObject` (UnityPy TypeTreeHelper.cpp:751-811).
    fn reference(
        &mut self,
        f: &mut Fields,
        r: &mut RefFields,
        tree: &Arc<Tree>,
        lx: ListIx,
        input: &mut Option<Result<Out, ReadError>>,
        opened: &mut Option<Open>,
    ) -> Step {
        let mut read = match input.take() {
            Some(out) => Some(tri!(out)),
            None => None,
        };
        'step: loop {
            let mut more = true;
            if let Some(v) = read.take() {
                more = tri!(self.set(f, tree, v));
                let slot = tree.info(f.pending).map(|ci| ci.slot);
                if more && slot.is_some() && slot == r.type_slot {
                    r.resolved = Resolved::Not;
                }
            }
            let kids = tree.list().children(f.node);
            while let Some(&c) = kids.get(f.next).filter(|_| more) {
                f.next += 1;
                visit();
                let Some(ci) = tree.info(c).copied() else {
                    continue;
                };
                let named_type = tree.list().name(c) == b"type";
                if r.type_slot.is_none() && named_type {
                    r.type_slot = Some(ci.slot);
                }
                if named_type {
                    r.object_type = self.progress.is_some() && ci.kind != Kind::ReferencedObject;
                }
                if ci.kind != Kind::ReferencedObjectData {
                    f.pending = c;
                    read = Some(read_child!(self, tree, lx, c, ci.repeated, opened));
                    continue 'step;
                }
                let found = if let Resolved::To(found) = &r.resolved {
                    found.clone()
                } else {
                    let Some(d) = f.dict.as_ref() else {
                        return Step::Done(Err(self.twice()));
                    };
                    let ty = r
                        .type_slot
                        .and_then(|s| d.position(s))
                        .and_then(|p| d.entries.as_slice().get(p))
                        .map(|(_, v)| v);
                    match self.ref_node(ty, r.object_type) {
                        Ok(found) => {
                            r.resolved = Resolved::To(found.clone());
                            found
                        }
                        Err(error) => {
                            if let Some(progress) = &mut self.progress {
                                tri!(progress.recover(error));
                                break;
                            }
                            return Step::Done(Err(error));
                        }
                    }
                };
                match found {
                    None | Some(RefNode::NoTree) => {
                        // A skipped child counts as a value (plan 3.10, "Bounds").
                        tri!(self.work.skipped(self.at()));
                    }
                    Some(RefNode::Tree(rt)) => {
                        let at = self.at();
                        let rlx = tri!(self.trees.index(&mut self.builder, &rt, at));
                        f.pending = c;
                        let root = rt.root();
                        read = Some(read_child!(self, &rt, rlx, root, ci.repeated, opened));
                        continue 'step;
                    }
                }
            }
            return self.finish(&mut f.dict, lx);
        }
    }

    fn list(&self, items: &mut MeteredVec<Value>) -> Step {
        let items = std::mem::replace(items, self.builder.vec());
        Step::Done(Ok(Out::Built(self.builder.list(items))))
    }

    /// A class's or a `ReferencedObject`'s end.
    fn finish(&mut self, dict: &mut Option<Dict>, lx: ListIx) -> Step {
        let Some(dict) = dict.take() else {
            return Step::Done(Err(self.twice()));
        };
        let result = tri!(dict.finish(&self.builder, lx, self.at()));
        if let Some(progress) = &mut self.progress {
            progress.finish_class();
        }
        Step::Done(Ok(result))
    }
}
