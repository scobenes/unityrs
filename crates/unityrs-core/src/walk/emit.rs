// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.

//! A direct read for a binding: UnityPyBoost's reader ([`super::boost`]'s
//! rules, R2-R15) over the trees and objects that need none of its
//! bookkeeping, each value handed to a [`Sink`] as it is read, with no
//! [`crate::value::Value`] in between (UnityPyBoost builds its Python
//! objects as it reads, `TypeTreeHelper.cpp:702-883`).
//!
//! It reads only what it can read exactly as [`super::boost`] does and
//! gives up (`Err`) on anything else, before or during the read: a tree
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

/// Where a primitive run's elements go ([`Run::each_into`]).
pub trait Elements {
    /// One element; `None` refuses it, and the rest.
    fn element(&mut self, v: Scalar) -> Option<()>;
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
    pub fn each(&self, f: impl FnMut(Scalar) -> Option<()>) -> Option<()> {
        struct Each<F>(F);
        impl<F: FnMut(Scalar) -> Option<()>> Elements for Each<F> {
            #[inline]
            fn element(&mut self, v: Scalar) -> Option<()> {
                (self.0)(v)
            }
        }
        self.each_into(&mut Each(f))
    }

    /// Each element in order into `to`, until it refuses one (`None`): as
    /// [`Self::each`], for a sink whose [`Elements::element`] is to be
    /// inlined into each kind's loop.
    #[inline]
    pub fn each_into<E: Elements>(&self, to: &mut E) -> Option<()> {
        let mut f = |v| to.element(v);
        let (big, floats) = (self.big, self.floats);
        macro_rules! each {
            // Each byte order its own loop, so it is not tested per
            // element.
            ($n:literal, |$b:ident, $big:ident| $conv:expr) => {{
                let chunks = self.bytes.as_chunks::<$n>().0;
                if big {
                    for $b in chunks {
                        let ($b, $big) = (*$b, true);
                        let _ = $big;
                        f($conv)?;
                    }
                } else {
                    for $b in chunks {
                        let ($b, $big) = (*$b, false);
                        let _ = $big;
                        f($conv)?;
                    }
                }
            }};
        }
        match self.kind {
            Kind::U8 => each!(1, |b, big| Scalar::Int(i64::from(b[0]))),
            Kind::Bool => each!(1, |b, big| Scalar::Bool(b[0] != 0)),
            Kind::S8 => each!(1, |b, big| Scalar::Int(i64::from(i8::from_le_bytes(b)))),
            Kind::U16 => each!(2, |b, big| Scalar::Int(i64::from(num!(big, u16, b)))),
            Kind::S16 => each!(2, |b, big| Scalar::Int(i64::from(num!(big, i16, b)))),
            Kind::U32 => each!(4, |b, big| Scalar::Int(i64::from(num!(big, u32, b)))),
            Kind::S32 => each!(4, |b, big| Scalar::Int(i64::from(num!(big, i32, b)))),
            Kind::U64 => each!(8, |b, big| Scalar::UInt(num!(big, u64, b))),
            Kind::S64 => each!(8, |b, big| Scalar::Int(num!(big, i64, b))),
            Kind::F32 => each!(4, |b, big| Scalar::Float(f64::from(f32::from_bits(
                f32_bits(num!(big, u32, b), floats)
            )))),
            Kind::F64 => each!(8, |b, big| Scalar::Float(f64::from_bits(num!(big, u64, b)))),
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
/// It does at most `max_work` work: it gives up on passing it. A read that
/// gives up returns the work it did until then.
pub fn emit<S: Sink>(
    data: &[u8],
    tree: &Tree,
    big: bool,
    floats: FloatMode,
    max_depth: u32,
    max_work: u64,
    sink: &mut S,
) -> Result<Emitted<S::Obj>, u64> {
    let mut r = Emit {
        cur: Cur {
            bytes: data,
            pos: 0,
            big,
            floats,
            work: 0,
            max_work,
        },
        tree,
        max_depth: max_depth as usize,
        frames: Vec::with_capacity(16),
        stack: Vec::with_capacity(64),
        sink,
    };
    let Some(value) = r.read(tree.root()) else {
        return Err(r.cur.work);
    };
    Ok(Emitted {
        value,
        bytes_read: r.cur.pos,
        work: r.cur.work,
    })
}

/// A value being read: a class or an array whose children are read in turn.
enum Frame<'a, O> {
    /// `read_class` (UnityPy TypeTreeHelper.cpp:394-445): child `next` of
    /// `kids`, the node's children, is read next.
    Class {
        node: NodeId,
        kids: &'a [NodeId],
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

/// A value's first look: read whole, or a frame opened for its children.
enum Entered<O> {
    Read(O),
    Opened,
}

/// What a value's first look found to open, its work counted and, for an
/// array, its length read.
#[derive(Clone, Copy)]
enum Open {
    Pair {
        align: bool,
    },
    Array {
        elem: NodeId,
        ei: NodeInfo,
        run: bool,
        count: usize,
        align: bool,
    },
    Class {
        align: bool,
    },
}

/// What a frame does next.
enum Step<O> {
    /// Read this node and hand its value back.
    Read(NodeId),
    /// The frame's value, complete.
    Done(O),
}

/// The read's place in the bytes and its work, apart from its frames, so a
/// class's children can be read while its frame is held.
struct Cur<'a> {
    bytes: &'a [u8],
    /// `ptr - start`: may pass the end after an alignment (R3).
    pos: usize,
    big: bool,
    floats: FloatMode,
    work: u64,
    /// The work it may do: past it, it gives up as soon as it passes it.
    max_work: u64,
}

struct Emit<'a, 's, S: Sink> {
    cur: Cur<'a>,
    tree: &'a Tree,
    max_depth: usize,
    /// The values being read, outermost first: the read's depth.
    frames: Vec<Frame<'a, S::Obj>>,
    /// The items of the arrays being read, each array's after its parent's.
    stack: Vec<S::Obj>,
    sink: &'s mut S,
}

impl Cur<'_> {
    /// R2: `ptr + n > end` fails.
    #[inline]
    fn bound(&self, n: usize) -> Option<usize> {
        self.pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
    }

    #[inline]
    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.bound(N)?;
        let b = <[u8; N]>::try_from(self.bytes.get(self.pos..end)?).ok()?;
        self.pos = end;
        Some(b)
    }

    /// The next `n` bytes, passed over.
    #[inline]
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.bound(n)?;
        let b = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(b)
    }

    /// R3: from the object's start, no bounds check.
    #[inline]
    fn align4(&mut self) {
        self.pos = self.pos.checked_add(3).map_or(usize::MAX & !3, |p| p & !3);
    }

    /// `_read_length`: a negative length gives up.
    #[inline]
    fn length(&mut self) -> Option<usize> {
        let n = num!(self.big, i32, self.fixed::<4>()?);
        usize::try_from(n).ok()
    }

    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "once per primitive value: outlined, it costs a call per value (callgrind)"
    )]
    fn prim(&mut self, kind: Kind) -> Option<Scalar> {
        let big = self.big;
        let int = Scalar::Int;
        Some(match kind {
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
        })
    }

    /// A primitive run: its bounds first, then each element.
    fn run<S: Sink>(
        &mut self,
        sink: &mut S,
        kind: Kind,
        width: u32,
        count: usize,
    ) -> Option<S::Obj> {
        let total = count.checked_mul(width as usize)?;
        let end = self.bound(total)?;
        // Counted once the sink takes the run: a run it refuses is no
        // work done.
        let work = self.work.saturating_add(total as u64);
        if work > self.max_work {
            return None;
        }
        let run = Run {
            kind,
            bytes: self.bytes.get(self.pos..end)?,
            count,
            big: self.big,
            floats: self.floats,
        };
        self.pos = end;
        let out = sink.run(run)?;
        self.work = work;
        Some(out)
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883) of node
    /// `me` up to its children: its value read whole (`Ok`), or what it
    /// opens (`Err`).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "once per value: inlined into the class loop it saves a call per value (callgrind)"
    )]
    fn look<S: Sink>(
        &mut self,
        tree: &Tree,
        me: &NodeInfo,
        sink: &mut S,
    ) -> Option<Result<S::Obj, Open>> {
        self.work = self.work.saturating_add(VALUE_WORK);
        if self.work > self.max_work {
            return None;
        }
        // R15: a flagged node aligns after its value.
        let mut align = me.align;
        let out = match me.shape {
            Shape::Prim => sink.scalar(self.prim(me.kind)?)?,
            // R6: always aligned.
            Shape::Str => {
                let len = self.length()?;
                let end = self.bound(len)?;
                let s = self.bytes.get(self.pos..end)?;
                self.pos = end;
                self.align4();
                sink.str(s)?
            }
            // R7: aligned only by its flag.
            Shape::Bytes => {
                let len = self.length()?;
                sink.bytes(self.take(len)?)?
            }
            Shape::Pair => return Some(Err(Open::Pair { align })),
            Shape::Vector {
                elem,
                run,
                array_align,
            } => {
                align |= array_align;
                let count = self.length()?;
                let ei = tree.info(elem).copied().unwrap_or(*me);
                // R10: a run aligns once after when its element is flagged.
                align |= run && ei.align;
                if run && let Some(width) = ei.kind.prim_size() {
                    self.run(sink, ei.kind, width, count)?
                } else {
                    return Some(Err(Open::Array {
                        elem,
                        ei,
                        run,
                        count,
                        align,
                    }));
                }
            }
            Shape::Class => return Some(Err(Open::Class { align })),
            Shape::BadPair | Shape::BadVector | Shape::RefObject => return None,
        };
        if align {
            self.align4();
        }
        Some(Ok(out))
    }

    /// Whether `count` elements of at least `min_size` bytes each can fit
    /// in what is left: if not, the full reader fails on them.
    fn fits(&self, count: usize, min_size: u32) -> bool {
        let left = self.bytes.len().saturating_sub(self.pos);
        min_size == 0 || count <= left / min_size as usize
    }
}

impl<'a, S: Sink> Emit<'a, '_, S> {
    /// Reads `root`'s value: each node entered ([`Self::enter`]) is either
    /// read whole or opens a frame, whose children are read in turn and
    /// handed to it ([`Self::accept`]) until it is complete ([`Self::step`]).
    /// A class's children read whole are set in its dict as they are read
    /// ([`Self::class_children`]).
    fn read(&mut self, root: NodeId) -> Option<S::Obj> {
        let mut value = match self.enter(root)? {
            Entered::Read(v) => return Some(v),
            Entered::Opened => None,
        };
        loop {
            if let Some(v) = value.take() {
                if self.frames.is_empty() {
                    return Some(v);
                }
                self.accept(v)?;
            }
            if self.class_children()? {
                continue;
            }
            match self.step()? {
                Step::Read(n) => {
                    if let Entered::Read(v) = self.enter(n)? {
                        value = Some(v);
                    }
                }
                Step::Done(v) => value = Some(v),
            }
        }
    }

    /// The innermost frame, where it is a class: its children read in
    /// turn, each one read whole set in its dict, up to the first that
    /// opens a frame (`true`, that frame now innermost) or the last.
    /// Each child as [`Self::step`] and [`Self::enter`] take it.
    #[inline]
    fn class_children(&mut self) -> Option<bool> {
        let depth = self.frames.len();
        let Self {
            cur,
            tree,
            frames,
            sink,
            max_depth,
            ..
        } = self;
        let Some(Frame::Class {
            kids, next, dict, ..
        }) = frames.last_mut()
        else {
            return Some(false);
        };
        let tree: &'a Tree = tree;
        while let Some(&c) = kids.get(*next) {
            let me = tree.info(c)?;
            if me.kind == Kind::ManagedReferencesRegistry || depth >= *max_depth {
                return None;
            }
            match cur.look(tree, me, &mut **sink)? {
                Ok(v) => {
                    *next += 1;
                    sink.set(dict, me.key, v)?;
                }
                Err(open) => {
                    let me = *me;
                    self.open(c, &me, open)?;
                    return Some(true);
                }
            }
        }
        Some(false)
    }

    /// `read_typetree_value` (UnityPy TypeTreeHelper.cpp:702-883) up to its
    /// children: a value read whole, or a frame opened.
    fn enter(&mut self, node: NodeId) -> Option<Entered<S::Obj>> {
        if self.frames.len() >= self.max_depth {
            return None;
        }
        let me = *self.tree.info(node)?;
        match self.cur.look(self.tree, &me, &mut *self.sink)? {
            Ok(v) => Some(Entered::Read(v)),
            Err(open) => {
                self.open(node, &me, open)?;
                Some(Entered::Opened)
            }
        }
    }

    /// Opens the frame `look` found for node `me`.
    fn open(&mut self, node: NodeId, me: &NodeInfo, open: Open) -> Option<()> {
        match open {
            Open::Pair { align } => {
                let (a, b) = two(self.tree, node);
                self.frames.push(Frame::Pair {
                    a,
                    b,
                    items: (None, None),
                    align,
                });
            }
            Open::Array {
                elem,
                ei,
                run,
                count,
                align,
            } => self.array(elem, ei, run, count, align)?,
            Open::Class { align } => {
                if me.dup_names {
                    return None;
                }
                let kids = self.tree.list().children(node);
                let dict = self.sink.dict(node, kids.len())?;
                self.frames.push(Frame::Class {
                    node,
                    kids,
                    next: 0,
                    dict,
                    align,
                });
            }
        }
        Some(())
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
            || !self.cur.fits(count, ei.min_size)
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
                kids, next, dict, ..
            } => {
                let c = *kids.get(*next)?;
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
            Frame::Class { kids, next, .. } => kids.get(*next).copied(),
            Frame::List { elem, left, .. } => (*left > 0).then_some(*elem),
            Frame::Pairs {
                a, b, left, first, ..
            } => match first {
                Some(_) => Some(*b),
                None if *left > 0 => {
                    // The element's own value, as UnityPyBoost's reader counts it.
                    self.cur.work = self.cur.work.saturating_add(VALUE_WORK);
                    if self.cur.work > self.cur.max_work {
                        return None;
                    }
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
            self.cur.align4();
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
