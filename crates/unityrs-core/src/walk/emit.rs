// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! A direct read for a binding: UnityPyBoost's reader ([`super::boost`]'s
//! rules, R2-R15) over the trees and objects that need none of its
//! bookkeeping, each value handed to a [`Sink`] as it is read, with no
//! [`crate::value::Value`] in between (UnityPyBoost builds its Python
//! objects as it reads, `TypeTreeHelper.cpp:702-883`).
//!
//! It reads only what it can read exactly as [`super::boost`] does and
//! gives up (`None`) on anything else, before or during the read: a tree
//! with a `ReferencedObject`, a `ManagedReferencesRegistry` or a class
//! whose children repeat a name ([`supported`]); a malformed `pair` or
//! `Array` met; a read past the end, a negative length, a depth past
//! `max_value_depth`, a list longer than its bytes can hold; a sink that
//! refuses. The caller then reads with the full reader, which gives the
//! outcome, error or value. So a read that comes back is one the full
//! reader completes with the same values: on these trees it counts no value
//! against `max_values_per_object` and no element as zero-size (nothing is
//! skipped or replaced, `Work`'s docs), and its only other bounds are the
//! open's work, which [`crate::ObjectRef::emit_typetree`] counts in one
//! check-and-add once the read is done, and memory, which the sink charges.
//!
//! The work is [`super::boost`]'s, to the byte: [`VALUE_WORK`] a value read
//! (a `pair` run's element too), and a primitive run's bytes.

use super::{FloatMode, VALUE_WORK, f32_bits};
use crate::typetree::{Kind, NodeId, NodeInfo, Shape, Tree};

macro_rules! num {
    ($big:expr, $t:ty, $b:expr) => {
        if $big {
            <$t>::from_be_bytes($b)
        } else {
            <$t>::from_le_bytes($b)
        }
    };
}

/// Where a direct read's values go. Every method may refuse (`None` or
/// `false`): the read then stops and gives up. The values are handed over
/// owned; a read that gives up drops those it holds.
pub trait Sink {
    /// One value.
    type Obj;
    /// A scalar.
    fn scalar(&mut self, v: Scalar) -> Option<Self::Obj>;
    /// A `string`'s raw bytes.
    fn str(&mut self, v: &[u8]) -> Option<Self::Obj>;
    /// `TypelessData`'s bytes.
    fn bytes(&mut self, v: &[u8]) -> Option<Self::Obj>;
    /// A `pair`.
    fn tuple(&mut self, first: Self::Obj, second: Self::Obj) -> Option<Self::Obj>;
    /// Class `node`'s dict, before its `children` are read.
    fn dict(&mut self, node: NodeId, children: usize) -> Option<Self::Obj>;
    /// Class `node`'s value, from its dict once every child is set: the
    /// dict itself, or an object a sink makes of it.
    fn close(&mut self, node: NodeId, dict: Self::Obj) -> Option<Self::Obj> {
        let _ = node;
        Some(dict)
    }
    /// `dict[name] = value`, the name `key`'s in the tree read.
    fn set(&mut self, dict: &Self::Obj, key: NodeId, value: Self::Obj) -> Option<()>;
    /// An array's list, of every item drained, in order.
    fn list(&mut self, items: std::vec::Drain<'_, Self::Obj>) -> Option<Self::Obj>;
    /// A primitive run's list, its bounds checked.
    fn run(&mut self, run: Run<'_>) -> Option<Self::Obj>;
    /// Room for an array of `items` elements, not a primitive run's, asked
    /// before any is read.
    fn room(&mut self, items: usize) -> bool;
}

/// One scalar of a primitive run ([`Run::each`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    /// `bool`.
    Bool(bool),
    /// An integer that fits `i64`.
    Int(i64),
    /// A `UInt64`, any of them.
    UInt(u64),
    /// A `float` (widened) or a `double`.
    Float(f64),
}

/// A primitive run's elements, in the file's order (`read_num_array`,
/// UnityPy TypeTreeHelper.cpp:86-264): exactly [`Self::len`] of them.
#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    kind: Kind,
    bytes: &'a [u8],
    count: usize,
    big: bool,
    floats: FloatMode,
}

impl Run<'_> {
    /// How many elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it has none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Each element in order, until `f` refuses one (`None`).
    #[inline]
    pub fn each(&self, mut f: impl FnMut(Scalar) -> Option<()>) -> Option<()> {
        let (big, floats) = (self.big, self.floats);
        macro_rules! each {
            ($n:literal, |$b:ident| $conv:expr) => {{
                for $b in self.bytes.as_chunks::<$n>().0 {
                    let $b = *$b;
                    f($conv)?;
                }
            }};
        }
        match self.kind {
            Kind::U8 => each!(1, |b| Scalar::Int(i64::from(b[0]))),
            Kind::Bool => each!(1, |b| Scalar::Bool(b[0] != 0)),
            Kind::S8 => each!(1, |b| Scalar::Int(i64::from(i8::from_le_bytes(b)))),
            Kind::U16 => each!(2, |b| Scalar::Int(i64::from(num!(big, u16, b)))),
            Kind::S16 => each!(2, |b| Scalar::Int(i64::from(num!(big, i16, b)))),
            Kind::U32 => each!(4, |b| Scalar::Int(i64::from(num!(big, u32, b)))),
            Kind::S32 => each!(4, |b| Scalar::Int(i64::from(num!(big, i32, b)))),
            Kind::U64 => each!(8, |b| Scalar::UInt(num!(big, u64, b))),
            Kind::S64 => each!(8, |b| Scalar::Int(num!(big, i64, b))),
            Kind::F32 => each!(4, |b| Scalar::Float(f64::from(f32::from_bits(f32_bits(
                num!(big, u32, b),
                floats
            ))))),
            Kind::F64 => each!(8, |b| Scalar::Float(f64::from_bits(num!(big, u64, b)))),
            _ => return None,
        }
        Some(())
    }
}

/// Whether a direct read can read `tree` at all ([`emit`] gives up on the
/// rest): no `ReferencedObject`, no `ManagedReferencesRegistry`, and no
/// class whose children repeat a name.
#[must_use]
pub fn supported(tree: &Tree) -> bool {
    let mut i = 0_u32;
    while let Some(info) = tree.info(NodeId(i)) {
        if info.shape == Shape::RefObject
            || info.kind == Kind::ManagedReferencesRegistry
            || info.dup_names
        {
            return false;
        }
        i += 1;
    }
    true
}

/// What a direct read gives back: its value, the bytes it read (from the
/// start of `data`; past its end after a final alignment, as UnityPyBoost's
/// `ptr - start`), and the open's work it did.
pub struct Emitted<O> {
    /// The root's value.
    pub value: O,
    /// `bytes_read`.
    pub bytes_read: usize,
    /// The work to count on the open's meter.
    pub work: u64,
}

/// Reads `tree`'s value from exactly `data` (the object's bytes) into
/// `sink`, or gives up (`None`): the module docs. It does not recurse: the
/// values being read are a list of frames on the heap, at most
/// `max_value_depth` of them, so a read takes the same stack at any depth.
pub fn emit<S: Sink>(
    data: &[u8],
    tree: &Tree,
    big: bool,
    floats: FloatMode,
    max_depth: u32,
    sink: &mut S,
) -> Option<Emitted<S::Obj>> {
    let mut r = Emit {
        bytes: data,
        pos: 0,
        big,
        floats,
        tree,
        work: 0,
        max_depth: max_depth as usize,
        frames: Vec::with_capacity(16),
        stack: Vec::with_capacity(64),
        sink,
    };
    let value = r.read(tree.root())?;
    Some(Emitted {
        value,
        bytes_read: r.pos,
        work: r.work,
    })
}

/// A value being read: a class or an array whose children are read in turn.
enum Frame<O> {
    /// `read_class` (UnityPy TypeTreeHelper.cpp:394-445): child `next` of
    /// the node's is read next.
    Class {
        node: NodeId,
        next: usize,
        dict: O,
        align: bool,
    },
    /// R11: elements read one by one, `left` of them still to read, the
    /// items read so far on the stack from `base`.
    List {
        elem: NodeId,
        left: usize,
        base: usize,
        align: bool,
    },
    /// A `pair` run (`read_pair_array`, UnityPy
    /// TypeTreeHelper.cpp:353-392): as a list, each element's first item
    /// held until its second is read.
    Pairs {
        a: NodeId,
        b: NodeId,
        left: usize,
        base: usize,
        first: Option<O>,
        align: bool,
    },
    /// A `pair`, its items as they are read.
    Pair {
        a: NodeId,
        b: NodeId,
        items: (Option<O>, Option<O>),
        align: bool,
    },
}

/// What a frame does next.
/// A value's first look: read whole, or a frame opened for its children.
enum Entered<O> {
    Read(O),
    Opened,
}

enum Step<O> {
    /// Read this node and hand its value back.
    Read(NodeId),
    /// The frame's value, complete.
    Done(O),
}

struct Emit<'a, S: Sink> {
    bytes: &'a [u8],
    /// `ptr - start`: may pass the end after an alignment (R3).
    pos: usize,
    big: bool,
    floats: FloatMode,
    tree: &'a Tree,
    work: u64,
    max_depth: usize,
    /// The values being read, outermost first: the read's depth.
    frames: Vec<Frame<S::Obj>>,
    /// The items of the arrays being read, each array's after its parent's.
    stack: Vec<S::Obj>,
    sink: &'a mut S,
}

impl<S: Sink> Emit<'_, S> {
    /// R2: `ptr + n > end` fails.
    fn bound(&self, n: usize) -> Option<usize> {
        self.pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
    }

    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.bound(N)?;
        let b = <[u8; N]>::try_from(self.bytes.get(self.pos..end)?).ok()?;
        self.pos = end;
        Some(b)
    }

    /// R3: from the object's start, no bounds check.
    fn align4(&mut self) {
        self.pos = self.pos.checked_add(3).map_or(usize::MAX & !3, |p| p & !3);
    }

    /// `_read_length`: a negative length gives up.
    fn length(&mut self) -> Option<usize> {
        let n = num!(self.big, i32, self.fixed::<4>()?);
        usize::try_from(n).ok()
    }

    fn prim(&mut self, kind: Kind) -> Option<S::Obj> {
        let big = self.big;
        let int = Scalar::Int;
        let v = match kind {
            Kind::U8 => int(i64::from(self.fixed::<1>()?[0])),
            Kind::S8 => int(i64::from(i8::from_le_bytes(self.fixed()?))),
            Kind::Bool => Scalar::Bool(self.fixed::<1>()?[0] != 0),
            Kind::U16 => int(i64::from(num!(big, u16, self.fixed()?))),
            Kind::S16 => int(i64::from(num!(big, i16, self.fixed()?))),
            Kind::U32 => int(i64::from(num!(big, u32, self.fixed()?))),
            Kind::S32 => int(i64::from(num!(big, i32, self.fixed()?))),
            Kind::U64 => Scalar::UInt(num!(big, u64, self.fixed()?)),
            Kind::S64 => int(num!(big, i64, self.fixed()?)),
            Kind::F32 => {
                let bits = f32_bits(num!(big, u32, self.fixed()?), self.floats);
                Scalar::Float(f64::from(f32::from_bits(bits)))
            }
            Kind::F64 => Scalar::Float(f64::from_bits(num!(big, u64, self.fixed()?))),
            _ => return None,
        };
        self.sink.scalar(v)
    }

    /// Reads `root`'s value: each node entered ([`Self::enter`]) is either
    /// read whole or opens a frame, whose children are read in turn and
    /// handed to it ([`Self::accept`]) until it is complete ([`Self::step`]).
    fn read(&mut self, root: NodeId) -> Option<S::Obj> {
        let mut node = Some(root);
        let mut value = None;
        loop {
            if let Some(n) = node.take()
                && let Entered::Read(v) = self.enter(n)?
            {
                value = Some(v);
            }
            if let Some(v) = value.take() {
                if self.frames.is_empty() {
                    return Some(v);
                }
                self.accept(v)?;
            }
            match self.step()? {
                Step::Read(n) => node = Some(n),
                Step::Done(v) => value = Some(v),
            }
        }
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883) up to its
    /// children: a value read whole, or a frame opened.
    fn enter(&mut self, node: NodeId) -> Option<Entered<S::Obj>> {
        if self.frames.len() >= self.max_depth {
            return None;
        }
        let me = *self.tree.info(node)?;
        self.work = self.work.saturating_add(VALUE_WORK);
        // R15: a flagged node aligns after its value.
        let mut align = me.align;
        let out = match me.shape {
            Shape::Prim => self.prim(me.kind)?,
            // R6: always aligned.
            Shape::Str => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let s = self.bytes.get(self.pos..end)?;
                self.pos = end;
                self.align4();
                self.sink.str(s)?
            }
            // R7: aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let b = self.bytes.get(self.pos..end)?;
                self.pos = end;
                self.sink.bytes(b)?
            }
            Shape::Pair => {
                let (a, b) = two(self.tree, node);
                self.frames.push(Frame::Pair {
                    a,
                    b,
                    items: (None, None),
                    align,
                });
                return Some(Entered::Opened);
            }
            Shape::Vector {
                elem,
                run,
                array_align,
            } => {
                align |= array_align;
                let count = self.length()?;
                let ei = self.tree.info(elem).copied().unwrap_or(me);
                // R10: a run aligns once after when its element is flagged.
                align |= run && ei.align;
                if run && let Some(width) = ei.kind.prim_size() {
                    self.run(ei.kind, width, count)?
                } else {
                    self.array(elem, ei, run, count, align)?;
                    return Some(Entered::Opened);
                }
            }
            Shape::Class => {
                if me.dup_names {
                    return None;
                }
                let children = self.tree.list().children(node).len();
                let dict = self.sink.dict(node, children)?;
                self.frames.push(Frame::Class {
                    node,
                    next: 0,
                    dict,
                    align,
                });
                return Some(Entered::Opened);
            }
            Shape::BadPair | Shape::BadVector | Shape::RefObject => return None,
        };
        if align {
            self.align4();
        }
        Some(Entered::Read(out))
    }

    /// Whether `count` elements of at least `min_size` bytes each can fit
    /// in what is left: if not, the full reader fails on them.
    fn fits(&self, count: usize, min_size: u32) -> bool {
        let left = self.bytes.len().saturating_sub(self.pos);
        min_size == 0 || count <= left / min_size as usize
    }

    /// A primitive run: its bounds first, then each element.
    fn run(&mut self, kind: Kind, width: u32, count: usize) -> Option<S::Obj> {
        let total = count.checked_mul(width as usize)?;
        let end = self.bound(total)?;
        self.work = self.work.saturating_add(total as u64);
        let run = Run {
            kind,
            bytes: self.bytes.get(self.pos..end)?,
            count,
            big: self.big,
            floats: self.floats,
        };
        self.pos = end;
        self.sink.run(run)
    }

    /// Opens the frame of an array read element by element: a `pair` run
    /// (`run`), else a list.
    fn array(
        &mut self,
        elem: NodeId,
        ei: NodeInfo,
        run: bool,
        count: usize,
        align: bool,
    ) -> Option<()> {
        if (run && ei.shape != Shape::Pair)
            || !self.fits(count, ei.min_size)
            || !self.sink.room(count)
        {
            return None;
        }
        let base = self.stack.len();
        self.frames.push(if run {
            let (a, b) = two(self.tree, elem);
            Frame::Pairs {
                a,
                b,
                left: count,
                base,
                first: None,
                align,
            }
        } else {
            Frame::List {
                elem,
                left: count,
                base,
                align,
            }
        });
        Some(())
    }

    /// Hands a child's value to the innermost frame.
    fn accept(&mut self, v: S::Obj) -> Option<()> {
        match self.frames.last_mut()? {
            Frame::Class {
                node, next, dict, ..
            } => {
                let c = *self.tree.list().children(*node).get(*next)?;
                let key = self.tree.info(c)?.key;
                *next += 1;
                self.sink.set(dict, key, v)
            }
            Frame::List { left, .. } => {
                *left -= 1;
                self.stack.push(v);
                Some(())
            }
            Frame::Pairs { left, first, .. } => {
                if let Some(first) = first.take() {
                    *left -= 1;
                    let pair = self.sink.tuple(first, v)?;
                    self.stack.push(pair);
                } else {
                    *first = Some(v);
                }
                Some(())
            }
            Frame::Pair { items, .. } => {
                if items.0.is_none() {
                    items.0 = Some(v);
                } else {
                    items.1 = Some(v);
                }
                Some(())
            }
        }
    }

    /// The innermost frame's next child, or its value once complete (the
    /// frame closed, its alignment applied).
    fn step(&mut self) -> Option<Step<S::Obj>> {
        let read = match self.frames.last_mut()? {
            Frame::Class { node, next, .. } => {
                let kids = self.tree.list().children(*node);
                kids.get(*next).copied()
            }
            Frame::List { elem, left, .. } => (*left > 0).then_some(*elem),
            Frame::Pairs {
                a, b, left, first, ..
            } => match first {
                Some(_) => Some(*b),
                None if *left > 0 => {
                    // The element's own value, as UnityPyBoost's reader counts it.
                    self.work = self.work.saturating_add(VALUE_WORK);
                    Some(*a)
                }
                None => None,
            },
            Frame::Pair { a, b, items, .. } => match items {
                (None, _) => Some(*a),
                (Some(_), None) => Some(*b),
                (Some(_), Some(_)) => None,
            },
        };
        if let Some(n) = read {
            if let Frame::Class { .. } = self.frames.last()? {
                let kind = self.tree.info(n)?.kind;
                if kind == Kind::ManagedReferencesRegistry {
                    return None;
                }
            }
            return Some(Step::Read(n));
        }
        let (value, align) = match self.frames.pop()? {
            Frame::Class {
                node, dict, align, ..
            } => (self.sink.close(node, dict)?, align),
            Frame::List { base, align, .. } | Frame::Pairs { base, align, .. } => {
                (self.sink.list(self.stack.drain(base..))?, align)
            }
            Frame::Pair {
                items: (first, second),
                align,
                ..
            } => (self.sink.tuple(first?, second?)?, align),
        };
        if align {
            self.align4();
        }
        Some(Step::Done(value))
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
