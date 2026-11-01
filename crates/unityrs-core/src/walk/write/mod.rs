//! UnityPy's pure-Python writer (`helpers/TypeTreeHelper.py:161-179`,
//! `:381-458`). Dispatch is on the type name first (`FUNCTION_WRITE_MAP`), then
//! `pair`, `ReferencedObject`, an `Array` first child, and a class. Under
//! [`super::WriteRules::UnityPy`] each array element aligns after itself when
//! its node is flagged, then the vector or `Array` flag aligns once; a class
//! writes `value[child.m_Name]` for every child, a missing key failing and an
//! extra key ignored; `ReferencedObject` data needs a non-empty `ref_types`
//! (`:99-100`) and a ref type with a tree: an empty class, or a matched ref
//! type with no tree, fails (`KeyError: 'data'` when the reader left `data`
//! out, else an `AttributeError` on the `None` node); integers are
//! range-checked as `struct.pack` checks them.
//!
//! **Duck typing (W2).** The writer takes any value Python's operations
//! accept, not only the variant the reader makes for the node, which a
//! duplicate-name object exercises (its dict holds one child's value under
//! the other's node). Probed on UnityPy 1.25.2: a `bool` node takes any value
//! (its truthiness; an empty `Str`, `Bytes`, `List`, `Tuple` or `Dict` is
//! false); an integer node takes `Bool` and `Int`/`UInt` in range; a `float`
//! or `double` node takes `Bool`, integers and floats; a `string` node only
//! `Str`; `TypelessData` only `Bytes`; a `pair` node anything with items 0
//! and 1 (`List`, `Tuple`, `Prims` or `Bytes` of length 2 or more, a `Str`
//! of 2 or more characters written through the children's kinds); an array
//! any sized iterable (`List`, `Tuple`, `Prims`, `Bytes` as integers, `Str`
//! as one-character strings, a `Dict` as its keys), each element written by
//! the element node under the same rules; a class with no children any value
//! (its non-dict branch iterates no children and writes nothing); a class
//! with children a `Dict` (`value[child.m_Name]`) or any other value that has
//! each child's cleaned name as a Python attribute (`getattr(value,
//! child._clean_name)`, `:447-455`: an `int` value satisfies a child named
//! `real`, a `list` one named `count`, whose bound method a `bool` node
//! writes as true). An `Array` node with three or more children writes its
//! elements through `m_Children[1]` (`:434`), as the pure-Python reader
//! reads them.
//!
//! Owner: package B.

use std::sync::Arc;

use bytes::Bytes;

use super::dict::{FindError, RefIndex, steps, visit};
use super::{
    OpenWork, RefTypes, VALUE_WORK, WriteCtx, WriteError, WriteRules, py_size_field_is_zero,
};
use crate::budget::DecodeBuf;
use crate::raw::RawStr;
use crate::typetree::{Kind, NodeId, Tree, TreeError};
use crate::value::{Map, Parsed, Value};

mod bounds;
mod lookup;
mod pyobj;

use lookup::{KeptTables, Lookup};
pub use pyobj::python_attributes;
use pyobj::{CLEAN_MAX, Obj, chars, clean_name, narrow};

/// Writes `parsed` through `tree` (the object's own tree, or a `nodes=`
/// override: keys are then matched by name), appending to `out`, a buffer
/// on the object's input's meter that charges before it grows (a value read
/// with a negative length can name the same bytes many times, so the output
/// can be far larger than the value; plan section 3.15). Its work is the
/// open's of `out`'s meter, counted as it goes whatever the outcome
/// (`OpenWork`): [`VALUE_WORK`] for each value it writes or child it
/// skips, but a primitive, which counts the bytes it writes, as a read counts
/// a primitive run's; and each step of finding a child's entry that builds
/// no value ([`super::STEP_WORK`], and the bytes of a name it compares or
/// hashes). A write is refused before it starts once the open's work has
/// passed its allowance, and fails once its own work takes the open past
/// it (the module docs of [`super`], "Work").
pub fn write(
    parsed: &Parsed,
    tree: &Tree,
    ctx: &WriteCtx<'_>,
    out: &mut DecodeBuf,
) -> Result<(), WriteError> {
    let meter = out.meter().clone();
    let open = OpenWork::start(&meter).map_err(WriteError::Limit)?;
    let base = out.len();
    let mut w = Writer {
        parsed,
        out,
        base,
        big: ctx.endian == crate::stream::Endian::Big,
        rules: ctx.rules,
        ref_types: ctx.ref_types,
        refs: RefIndex::new(ctx.ref_types),
        max_depth: ctx.limits.max_value_depth,
        limits: *ctx.limits,
        depth: 0,
        values: 0,
        repeated: 0,
        open,
        has_registry: false,
        shared: 0,
        kept: KeptTables::new(parsed.charge().meter()),
    };
    let written = w.value(tree, tree.root(), Obj::V(parsed.value()));
    let ended = w.open.finish();
    drop(w);
    written?;
    ended.map_err(WriteError::Limit)
}

struct Writer<'a, 'o> {
    parsed: &'a Parsed,
    out: &'o mut DecodeBuf,
    /// Where this object's bytes start in `out` (`align_stream` aligns the
    /// writer's own stream, which starts at the object).
    base: usize,
    big: bool,
    rules: WriteRules,
    ref_types: RefTypes<'a>,
    /// The ref types' search, one per write ([`RefIndex`]).
    refs: RefIndex<'a>,
    limits: crate::limits::Limits,
    max_depth: u32,
    depth: u32,
    /// What counts against `max_values_per_object` ([`Self::count`]).
    values: u64,
    /// How many children whose name a sibling repeats are being written
    /// ([`crate::typetree::NodeInfo::repeated`]).
    repeated: u32,
    /// The open's work, counted as the write goes.
    open: OpenWork<'o>,
    has_registry: bool,
    /// How many classes whose children repeat a name (`dup_names`) are
    /// being written: inside one, a dict can be written once a sibling,
    /// and its name table is kept ([`KeptTables`]).
    shared: u32,
    kept: KeptTables<'a>,
}

/// What a `ReferencedObject`'s data child writes through, resolved once a
/// write of the dict.
#[derive(Clone)]
enum Data {
    /// Nothing: [`WriteRules::MirrorReader`] writes no data the reader
    /// skipped.
    Skip,
    /// The matched ref type's tree, or none (the write then fails).
    Through(Option<Arc<Tree>>),
}

impl<'a> Writer<'a, '_> {
    fn name(tree: &Tree, node: NodeId) -> RawStr {
        tree.list()
            .row(node)
            .map(|r| r.name.clone())
            .unwrap_or_default()
    }

    fn mismatch(tree: &Tree, node: NodeId, expected: &'static str) -> WriteError {
        WriteError::TypeMismatch {
            node: Self::name(tree, node),
            expected,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), WriteError> {
        self.out
            .extend_from_slice(bytes)
            .map_err(WriteError::Charge)
    }

    /// Counts a `str`'s bytes as the open's work before a pass over them
    /// finds its characters (UTF-8, each byte of no character one escape),
    /// to measure or index it by character: a repeated name hands one
    /// string to any number of `pair` or array siblings, each of which made
    /// that pass uncounted, in every write (review round 6's sweep).
    fn str_work(&mut self, o: Obj<'_>) -> Result<(), WriteError> {
        let n = match o {
            Obj::V(Value::Str(s)) => s.len(),
            Obj::Str(s) => s.len(),
            _ => return Ok(()),
        };
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        self.open.add(n).map_err(WriteError::Limit)
    }

    /// `o[i]` ([`Self::item`]), a `str`'s pass counted first.
    fn index(&mut self, o: Obj<'a>, i: usize) -> Result<Option<Obj<'a>>, WriteError> {
        self.str_work(o)?;
        Ok(Self::item(o, i))
    }

    /// Puts a string's or a byte array's bytes, each a byte of the open's
    /// work, counted first: a read holds them as a slice of its input, in
    /// one step, and a write copies them, so K writes of objects over one
    /// long string copied it K times on [`VALUE_WORK`] each (review round
    /// 6's sweep).
    fn put_counted(&mut self, bytes: &[u8]) -> Result<(), WriteError> {
        let n = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.open.add(n).map_err(WriteError::Limit)?;
        self.put(bytes)
    }

    /// `align_stream` (`EndianBinaryWriter.py:94-97`): zeros to a multiple
    /// of 4 from the object's start.
    fn align(&mut self) -> Result<(), WriteError> {
        let pos = self.out.len().saturating_sub(self.base);
        let pad = (4 - pos % 4) % 4;
        self.put([0; 4].get(..pad).unwrap_or_default())
    }

    fn write_int(&mut self, n: usize, tree: &Tree, node: NodeId) -> Result<(), WriteError> {
        let n = i32::try_from(n).map_err(|_| WriteError::OutOfRange {
            node: Self::name(tree, node),
        })?;
        let b = if self.big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        };
        self.put(&b)
    }

    /// `d[name]` on a dict value, by key string: inside a class whose
    /// children repeat a name, by the dict's kept table (the dict may be
    /// written once a sibling); else by a scan, which each write of a
    /// `ReferencedObject` makes once ([`Self::ref_object`]). Each entry the
    /// scan passes is a step of the open's work, counted first.
    fn field(&mut self, m: &'a Map, name: &[u8]) -> Result<Option<&'a Value>, WriteError> {
        let pos = if self.shared > 0 {
            steps(1);
            self.kept.position(self.parsed, m, name, &mut self.open)?
        } else {
            let mut found = None;
            for (i, (n, _)) in self.parsed.entries(m).enumerate() {
                steps(1);
                self.open.steps(1).map_err(WriteError::Limit)?;
                if n == name {
                    found = Some(i);
                    break;
                }
            }
            found
        };
        Ok(pos.and_then(|p| m.entries().get(p)).map(|(_, v)| v))
    }

    /// The children of a class or `ReferencedObject` `node`, written by `f`;
    /// a node whose children repeat a name keeps the name tables built
    /// inside it until the outermost such node is written ([`KeptTables`]).
    fn children(
        &mut self,
        tree: &Tree,
        node: NodeId,
        f: impl FnOnce(&mut Self) -> Result<(), WriteError>,
    ) -> Result<(), WriteError> {
        let dup = tree.info(node).is_some_and(|i| i.dup_names);
        if dup {
            self.shared = self.shared.saturating_add(1);
        }
        // One call of `f`, so it inlines here: a frame less a level.
        let r = f(self);
        if dup {
            self.shared = self.shared.saturating_sub(1);
            if self.shared == 0 {
                self.kept = KeptTables::new(self.parsed.charge().meter());
            }
        }
        r
    }

    /// `child`'s entry by `find`, with the kept tables inside a class whose
    /// children repeat a name ([`Lookup::get`]).
    fn find(
        &mut self,
        find: &mut Lookup<'a>,
        tree: &Tree,
        child: NodeId,
    ) -> Result<Option<&'a Value>, WriteError> {
        let kept = (self.shared > 0).then_some(&mut self.kept);
        find.get(self.parsed, tree, child, kept, &mut self.open)
    }

    fn value(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        self.count(Self::cost(tree, node))?;
        self.frame(|w| w.value_inner(tree, node, o))
    }

    /// The open's work of writing `node`'s value ([`write`]): its bytes for
    /// a primitive, [`VALUE_WORK`] for any other value.
    fn cost(tree: &Tree, node: NodeId) -> u64 {
        tree.py_kind(node).prim_size().map_or(VALUE_WORK, u64::from)
    }

    /// `write_value` (`TypeTreeHelper.py:407-458`). The branches that
    /// recurse ([`Self::class`], [`Self::class_dict`], [`Self::ref_object`])
    /// are never inlined here, so a level's frame holds only its own
    /// branch's locals: inlined, every class level carried a
    /// `ReferencedObject`'s too (`tests/walk_stack.rs`'s `stack_measured`,
    /// release: 1,011,712 bytes for the deepest class write, 905,216 out of
    /// line, with toolchain 1.99.1).
    fn value_inner(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let mut align = tree.info(node).is_some_and(|i| i.align);
        let kind = tree.py_kind(node);
        let kids = tree.list().children(node);
        if kind.prim_size().is_some() {
            self.prim(tree, node, kind, o)?;
        } else if kind == Kind::Str {
            // `write_aligned_string` (`EndianBinaryWriter.py:88-92`), W3.
            let s = Self::as_str(o).ok_or_else(|| Self::mismatch(tree, node, "str"))?;
            self.write_int(s.len(), tree, node)?;
            self.put_counted(s)?;
            self.align()?;
        } else if kind == Kind::Bytes {
            // `write_byte_array` (`:110-112`), W4.
            let Obj::V(Value::Bytes(b)) = o else {
                return Err(Self::mismatch(tree, node, "bytes"));
            };
            self.write_int(b.len(), tree, node)?;
            self.put_counted(b)?;
        } else if kind == Kind::Pair {
            // `:419-421`: `value[0]`, `value[1]` through the first two children.
            for i in 0..2 {
                let item = self
                    .index(o, i)?
                    .ok_or_else(|| Self::mismatch(tree, node, "a sequence of two"))?;
                let child = kids
                    .get(i)
                    .copied()
                    .ok_or_else(|| WriteError::Malformed(Self::name(tree, node)))?;
                self.value(tree, child, item)?;
            }
        } else if kind == Kind::ReferencedObject {
            self.ref_object(tree, node, o)?;
        } else if let Some(array) = kids
            .first()
            .copied()
            .filter(|c| tree.py_kind(*c) == Kind::Array)
        {
            // `:429-435`, W5.
            align |= tree.info(array).is_some_and(|i| i.align);
            self.str_work(o)?;
            let n = Self::len(o).ok_or_else(|| Self::mismatch(tree, node, "a sized iterable"))?;
            self.write_int(n, tree, node)?;
            let elem = tree
                .list()
                .children(array)
                .get(1)
                .copied()
                .ok_or_else(|| WriteError::Malformed(Self::name(tree, node)))?;
            self.frame(|w| w.elements(tree, elem, o))?;
        } else {
            self.class(tree, node, o)?;
        }
        if align {
            self.align()?;
        }
        Ok(())
    }

    /// Each element of `o` through `elem` (`:435`). Under
    /// [`WriteRules::MirrorReader`] a primitive or `pair` element does not
    /// align itself; the run aligns once after, as UnityPyBoost reads it.
    fn elements(&mut self, tree: &Tree, elem: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let mirror_run =
            self.rules == WriteRules::MirrorReader && tree.py_kind(elem).reads_as_run();
        let elem_align = tree.info(elem).is_some_and(|i| i.align);
        let one = |w: &mut Self, item: Obj<'a>| -> Result<(), WriteError> {
            visit();
            if mirror_run {
                w.value_unaligned(tree, elem, item)
            } else {
                w.value(tree, elem, item)
            }
        };
        self.str_work(o)?;
        match o {
            Obj::V(Value::Str(s)) => {
                for c in chars(s.as_bytes()) {
                    one(self, Obj::Str(c))?;
                }
            }
            Obj::Str(s) => {
                for c in chars(s) {
                    one(self, Obj::Str(c))?;
                }
            }
            Obj::V(Value::Bytes(b)) => {
                for x in b {
                    one(self, Obj::Int(i128::from(*x)))?;
                }
            }
            Obj::V(Value::Prims(p)) => {
                for i in 0..p.len() {
                    if let Some(item) = Self::prim_item(p, i) {
                        one(self, item)?;
                    }
                }
            }
            Obj::V(Value::List(l)) => {
                for v in l {
                    one(self, Obj::V(v))?;
                }
            }
            Obj::V(Value::Tuple(t)) => {
                for v in t {
                    one(self, Obj::V(v))?;
                }
            }
            Obj::V(Value::Dict(m)) => {
                for (k, _) in m.entries() {
                    one(self, Obj::Str(self.parsed.key_name(m, *k)))?;
                }
            }
            _ => return Err(Self::mismatch(tree, elem, "a sized iterable")),
        }
        if mirror_run && elem_align {
            self.align()?;
        }
        Ok(())
    }

    /// An element of a run under [`WriteRules::MirrorReader`]: its value
    /// without its own trailing alignment.
    fn value_unaligned(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        self.count(Self::cost(tree, node))?;
        let kind = tree.py_kind(node);
        if kind.prim_size().is_some() {
            return self.prim(tree, node, kind, o);
        }
        let kids = tree.list().children(node);
        for i in 0..2 {
            let item = self
                .index(o, i)?
                .ok_or_else(|| Self::mismatch(tree, node, "a sequence of two"))?;
            let child = kids
                .get(i)
                .copied()
                .ok_or_else(|| WriteError::Malformed(Self::name(tree, node)))?;
            self.value(tree, child, item)?;
        }
        Ok(())
    }

    /// `FUNCTION_WRITE_MAP`'s numbers (`TypeTreeHelper.py:381-404`): an
    /// integer node takes an `int` or `bool` in its range, a float node a
    /// number, `bool` any object's truth.
    fn prim(
        &mut self,
        tree: &Tree,
        node: NodeId,
        kind: Kind,
        o: Obj<'a>,
    ) -> Result<(), WriteError> {
        let big = self.big;
        let range = || WriteError::OutOfRange {
            node: Self::name(tree, node),
        };
        // `struct.pack`'s bytes, built on the stack: a write charges only
        // its output.
        let mut buf = [0_u8; 8];
        macro_rules! put {
            ($v:expr) => {{
                let v = $v;
                let b = if big {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                };
                let n = b.len();
                buf.get_mut(..n).unwrap_or_default().copy_from_slice(&b);
                n
            }};
        }
        macro_rules! int {
            ($t:ty) => {{
                let i = Self::as_int(o).ok_or_else(|| Self::mismatch(tree, node, "int"))?;
                put!(<$t>::try_from(i).map_err(|_| range())?)
            }};
        }
        let n = match kind {
            // `pack('?', value)`: the object's truth.
            Kind::Bool => put!(u8::from(Self::truthy(o))),
            Kind::F32 => put!(match o {
                // A float read from a `float` node packs back to its bits.
                Obj::V(Value::F32(b)) => *b,
                Obj::F32(b) => b,
                _ => {
                    let x =
                        Self::as_double(o).ok_or_else(|| Self::mismatch(tree, node, "float"))?;
                    narrow(x).ok_or_else(range)?
                }
            }),
            Kind::F64 => {
                let x = Self::as_double(o).ok_or_else(|| Self::mismatch(tree, node, "float"))?;
                put!(x.to_bits())
            }
            Kind::S8 => int!(i8),
            Kind::U8 => int!(u8),
            Kind::S16 => int!(i16),
            Kind::U16 => int!(u16),
            Kind::S32 => int!(i32),
            Kind::U32 => int!(u32),
            Kind::S64 => int!(i64),
            _ => int!(u64),
        };
        let (bytes, _) = buf.split_at(n.min(8));
        // Every caller counted this value first, its bytes as its work
        // ([`Self::cost`]).
        self.put(bytes)
    }

    /// The class branch (`:437-455`), W6: `value[child.m_Name]` for a dict
    /// ([`Self::class_dict`]), `getattr(value, child._clean_name)` otherwise;
    /// the registry scope as on read.
    ///
    /// A class written from a value that is not a dict counts against
    /// `max_values_per_object` whatever its name: its children are the
    /// value's attributes, which the value's size does not bound (an `int`
    /// element's `real`, written through a class of any number of children
    /// for each element of an array).
    #[inline(never)]
    fn class(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        if let Obj::V(Value::Dict(m)) = o {
            return self.class_dict(tree, node, m);
        }
        self.bound()?;
        let outer = self.has_registry;
        let kids = tree.list().children(node);
        for c in kids {
            visit();
            if tree.py_kind(*c) == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    self.skip()?;
                    continue;
                }
                self.has_registry = true;
            }
            let name = tree.list().name(*c);
            let mut buf = [0_u8; CLEAN_MAX];
            let clean = clean_name(name, &mut buf).and_then(|n| buf.get(..n));
            let attr = match clean {
                Some(clean) => Self::getattr(o, clean),
                None => Ok(None),
            };
            let item = match attr {
                Ok(Some(a)) => a,
                Ok(None) => {
                    return Err(Self::mismatch(tree, *c, "an object with this attribute"));
                }
                Err(()) => {
                    return Err(Self::mismatch(
                        tree,
                        *c,
                        "a modelled (non-dunder) attribute",
                    ));
                }
            };
            let repeated = tree.info(*c).is_some_and(|i| i.repeated);
            self.child(repeated, |w| w.value(tree, *c, item))?;
        }
        self.has_registry = outer;
        Ok(())
    }

    /// The class branch on a dict (`:438-446`): `value[child.m_Name]` for
    /// each child, found by a [`Lookup`].
    #[inline(never)]
    fn class_dict(&mut self, tree: &Tree, node: NodeId, m: &'a Map) -> Result<(), WriteError> {
        let outer = self.has_registry;
        let mut find = Lookup::new(self.parsed, tree, node, m);
        self.children(tree, node, |w| {
            for c in tree.list().children(node) {
                visit();
                if tree.py_kind(*c) == Kind::ManagedReferencesRegistry {
                    if w.has_registry {
                        w.skip()?;
                        continue;
                    }
                    w.has_registry = true;
                }
                let v = w
                    .find(&mut find, tree, *c)?
                    .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
                let repeated = tree.info(*c).is_some_and(|i| i.repeated);
                w.child(repeated, |w| w.value(tree, *c, Obj::V(v)))?;
            }
            Ok(())
        })?;
        self.has_registry = outer;
        Ok(())
    }

    /// `ReferencedObject` (`:422-428`), W7: each child's entry found by a
    /// [`Lookup`]; the data's ref type resolved once a write of the dict,
    /// which does not change while it is written (claude X, review round 2:
    /// a node of many data children resolved it for each).
    #[inline(never)]
    fn ref_object(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let kids = tree.list().children(node);
        if kids.is_empty() {
            return Ok(());
        }
        let Obj::V(Value::Dict(m)) = o else {
            return Err(Self::mismatch(tree, node, "dict"));
        };
        let mut find = Lookup::new(self.parsed, tree, node, m);
        let mut data: Option<Data> = None;
        self.children(tree, node, |w| {
            for c in kids {
                visit();
                let repeated = tree.info(*c).is_some_and(|i| i.repeated);
                if tree.py_kind(*c) != Kind::ReferencedObjectData {
                    let v = w
                        .find(&mut find, tree, *c)?
                        .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
                    w.child(repeated, |w| w.value(tree, *c, Obj::V(v)))?;
                    continue;
                }
                let through = if let Some(d) = &data {
                    d.clone()
                } else {
                    let d = match w.rules {
                        WriteRules::UnityPy => Data::Through(w.ref_node_py(m)?),
                        WriteRules::MirrorReader => match w.ref_node_boost(m)? {
                            // The reader skipped the data: so does the mirror.
                            None => Data::Skip,
                            Some(t) => Data::Through(Some(t)),
                        },
                    };
                    data.insert(d).clone()
                };
                let Data::Through(ref_tree) = through else {
                    w.skip()?;
                    continue;
                };
                // `value[child.m_Name]`, then `write_value(..., None)` raises.
                let v = w
                    .find(&mut find, tree, *c)?
                    .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
                let Some(rt) = ref_tree else {
                    return Err(Self::mismatch(tree, *c, "a ref type with a tree"));
                };
                w.child(repeated, |w| w.value(&rt, rt.root(), Obj::V(v)))?;
            }
            Ok(())
        })
    }

    /// A matched ref type whose tree does not compile.
    fn tree_error(e: TreeError) -> WriteError {
        match e {
            TreeError::Limit(l) => WriteError::Limit(l),
            TreeError::Charge(c) => WriteError::Charge(c),
            _ => WriteError::Malformed(RawStr::new(Bytes::from_static(b"ReferencedObjectData"))),
        }
    }

    /// The type dict's `class`, `ns` and `asm` (`ref_object["type"]`, then
    /// `typ["class"]`...; a type that is not a dict has no `class`
    /// attribute).
    fn names(&mut self, m: &'a Map) -> Result<(&'a Value, &'a Value, &'a Value), WriteError> {
        let key = |k: &'static [u8]| RawStr::new(Bytes::from_static(k));
        let t = match self.field(m, b"type")? {
            None => return Err(WriteError::MissingKey(key(b"type"))),
            Some(Value::Dict(t)) => t,
            Some(_) => {
                return Err(WriteError::TypeMismatch {
                    node: key(b"type"),
                    expected: "a dict or an object with class, ns and asm",
                });
            }
        };
        let mut get = |k: &'static [u8]| {
            self.field(t, k)?
                .ok_or_else(|| WriteError::MissingKey(key(k)))
        };
        Ok((get(b"class")?, get(b"ns")?, get(b"asm")?))
    }

    /// The first ref type whose names equal the three ([`RefIndex`]).
    fn find_ref(
        &mut self,
        (cls, ns, asm): (&Value, &Value, &Value),
    ) -> Result<Option<Arc<Tree>>, WriteError> {
        let r = self
            .refs
            .find(self.parsed.charge().meter(), &mut self.open, cls, ns, asm)
            .map_err(|e| match e {
                FindError::Charge(e) => WriteError::Charge(e),
                FindError::Work(l) => WriteError::Limit(l),
            })?
            .ok_or(WriteError::RefTypeNotFound)?;
        r.tree(&self.limits).transpose().map_err(Self::tree_error)
    }

    /// The pure-Python `get_ref_type_node` (`TypeTreeHelper.py:88-109`):
    /// `None` for an empty class; the writer then fails on it.
    fn ref_node_py(&mut self, m: &'a Map) -> Result<Option<Arc<Tree>>, WriteError> {
        let (cls, ns, asm) = self.names(m)?;
        if !matches!(self.ref_types, RefTypes::List(l) if !l.is_empty()) {
            return Err(WriteError::NoRefTypes);
        }
        if matches!(cls, Value::Str(s) if s.as_bytes().is_empty()) {
            return Ok(None);
        }
        self.find_ref((cls, ns, asm))
    }

    /// UnityPyBoost's resolution (R14), for [`WriteRules::MirrorReader`]:
    /// `None` where the reader skipped the data.
    fn ref_node_boost(&mut self, m: &'a Map) -> Result<Option<Arc<Tree>>, WriteError> {
        let RefTypes::List(_) = self.ref_types else {
            return Err(WriteError::NoRefTypes);
        };
        let (cls, ns, asm) = self.names(m)?;
        if py_size_field_is_zero(cls) {
            return Ok(None);
        }
        self.find_ref((cls, ns, asm))
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod work_tests;
