// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPy/streams/EndianBinaryReader.py, UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.
//! An array's elements (`TypeTreeHelper.py:435`): each written by
//! `write_value`, which aligns after every element whose node is flagged;
//! or, under [`WriteRules::MirrorReader`], as the reader that read the value
//! reads a run, aligned once after it.
//!
//! [`WriteRules::MirrorReader`]: super::WriteRules::MirrorReader

use super::{Obj, VALUE_WORK, WriteError, Writer, chars, visit};
use crate::typetree::{Kind, NodeId, Tree};
use crate::value::{ReadBy, Value};

impl<'a> Writer<'a, '_> {
    /// Each element of `o` through `elem` (`:435`). Under
    /// [`super::WriteRules::MirrorReader`] the elements of a run the reader
    /// reads as one are written without their own trailing alignment, and
    /// the run, when `elem` is flagged, aligns once after: for a value
    /// UnityPyBoost read, a primitive or `pair` run (`read_typetree_value_array`,
    /// `UnityPyBoost/TypeTreeHelper.cpp:886-940`); for one the pure-Python
    /// reader read, every flagged element (`read_value_array`,
    /// `TypeTreeHelper.py:266-361`, [`Self::py_element`]).
    pub(super) fn elements(
        &mut self,
        tree: &Tree,
        elem: NodeId,
        o: Obj<'a>,
    ) -> Result<(), WriteError> {
        let elem_align = tree.info(elem).is_some_and(|i| i.align);
        let run = match self.mirror {
            None => false,
            Some(ReadBy::Boost) => self.kind(tree, elem).reads_as_run(),
            Some(ReadBy::Python) => elem_align,
        };
        let one = |w: &mut Self, item: Obj<'a>| -> Result<(), WriteError> {
            visit();
            if run {
                w.run_element(tree, elem, item)
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
        if run && elem_align {
            self.align()?;
        }
        Ok(())
    }

    /// An element of a run under [`super::WriteRules::MirrorReader`]: its
    /// value without its own trailing alignment, as the reader that read the
    /// value reads it.
    fn run_element(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        self.count(self.cost(tree, node))?;
        if self.mirror == Some(ReadBy::Python) {
            return self.py_element(tree, node, o);
        }
        let kind = self.kind(tree, node);
        if kind.prim_size().is_some() {
            return self.prim(tree, node, kind, o);
        }
        self.two(tree, node, o, Self::value)
    }

    /// Items 0 and 1 of `o` through `node`'s first two children, each by
    /// `each` (a `pair`'s, `:419-421`).
    fn two(
        &mut self,
        tree: &Tree,
        node: NodeId,
        o: Obj<'a>,
        each: fn(&mut Self, &Tree, NodeId, Obj<'a>) -> Result<(), WriteError>,
    ) -> Result<(), WriteError> {
        let kids = tree.list().children(node);
        for i in 0..2 {
            let item = self
                .index(o, i)?
                .ok_or_else(|| Self::mismatch(tree, node, "a sequence of two"))?;
            let child = kids
                .get(i)
                .copied()
                .ok_or_else(|| WriteError::Malformed(Self::name(tree, node)))?;
            each(self, tree, child, item)?;
        }
        Ok(())
    }

    /// An element of a flagged run, as the pure-Python reader's
    /// `read_value_array` reads it (`TypeTreeHelper.py:266-361`), already
    /// counted: a primitive by its `struct` format, whose 16-bit signedness
    /// is swapped (`read_short_array` unpacks `H`, `read_u_short_array` `h`,
    /// `EndianBinaryReader.py:176-180`); a string by its function, so an
    /// empty one unaligned (`read_aligned_string`); bytes by
    /// `FUNCTION_READ_MAP`'s function, though no value this reader read
    /// through this node holds one (`read_value_array` calls `read_bytes()`
    /// without its length, `TypeTreeHelper.py:280`, and raises on a first
    /// element, so the run it read was empty); a `pair`'s children by
    /// [`Self::direct`]; a `ReferencedObject` or a
    /// class with each child's own flag, a class with no registry check
    /// (`:314-318`); a vector as its length and its elements, through
    /// `read_value_array` again when its own element is flagged (`:306-313`).
    /// None aligns after itself: the run does, once.
    fn py_element(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let kind = self.kind(tree, node);
        if kind.prim_size().is_some() {
            let format = match kind {
                Kind::S16 => Kind::U16,
                Kind::U16 => Kind::S16,
                k => k,
            };
            return self.prim(tree, node, format, o);
        }
        match kind {
            Kind::Str => return self.string(tree, node, o),
            Kind::Bytes => return self.bytes(tree, node, o),
            Kind::Pair => return self.two(tree, node, o, Self::direct),
            _ => {}
        }
        self.frame(|w| {
            if kind == Kind::ReferencedObject {
                return w.ref_object(tree, node, o);
            }
            let kids = tree.list().children(node);
            let array = kids
                .first()
                .copied()
                .filter(|c| w.kind(tree, *c) == Kind::Array);
            let Some(array) = array else {
                return w.class(tree, node, o, false);
            };
            w.str_work(o)?;
            let n = Self::len(o).ok_or_else(|| Self::mismatch(tree, node, "a sized iterable"))?;
            w.write_int(n, tree, node)?;
            let sub = tree
                .list()
                .children(array)
                .get(1)
                .copied()
                .ok_or_else(|| WriteError::Malformed(Self::name(tree, node)))?;
            w.frame(|w| w.elements(tree, sub, o))
        })
    }

    /// A child of a flagged `pair` run's element, as `read_value_array`
    /// reads it: `FUNCTION_READ_MAP.get(child.m_Type, lambda:
    /// read_value(child))` (`TypeTreeHelper.py:285-292`), so a primitive,
    /// `string` or `TypelessData` child by its function, without its own
    /// align flag (a non-empty string still aligns itself), any other
    /// through [`Self::value`], which keeps its flag.
    fn direct(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let kind = self.kind(tree, node);
        if let Some(size) = kind.prim_size() {
            self.count(u64::from(size))?;
            return self.prim(tree, node, kind, o);
        }
        match kind {
            Kind::Str => {
                self.count(VALUE_WORK)?;
                self.string(tree, node, o)
            }
            Kind::Bytes => {
                self.count(VALUE_WORK)?;
                self.bytes(tree, node, o)
            }
            _ => self.value(tree, node, o),
        }
    }
}
