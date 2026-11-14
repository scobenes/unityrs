// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp's object mode (clean_name, parse_class) and the attrs fields of UnityPy/classes/generated.py's Mesh, Sprite, VertexData, ChannelInfo, StreamInfo, SubMesh, StreamingInfo, CompressedMesh, PackedBitVector, BoneWeights4, BoneInfluence and SpriteRenderData, as MeshHandler reads them. See THIRD_PARTY_LICENSES.
//! An object-mode view: a dict-mode value the core read, viewed as the
//! objects UnityPy's object mode builds from it.
//!
//! UnityPy hands `MeshHandler` an object (`obj.read()`), which UnityPyBoost
//! builds class by class (`parse_class`, `UnityPyBoost/TypeTreeHelper.cpp:458-583`;
//! `PARITY.md` section 9.2): each class node's fields are its children's
//! clean names (`clean_name`, `:28-65`), and the class named by the node's
//! type is called with them. A class whose required field is missing, or a
//! slotted class (every class here but `Mesh` and `Sprite`, `PARITY.md`
//! section 9.2.1) given a field it lacks, falls back to an `UnknownObject`
//! holding the fields as attributes. So a field the handler reads is the
//! value of the last child of that clean name, and a missing one is the
//! field's default (`None`) on a class that was built and an
//! `AttributeError` on an `UnknownObject` ([`Obj::get`]).
//!
//! The core's value is a dict keyed by each child's raw name (the first
//! child of a name holding its last value), so [`Obj`] finds the child by
//! clean name in the node list and its value under its raw name, never
//! copying.
//!
//! **What is refused** ([`ViewError::Malformed`], Tier U): a node whose
//! type is not the class its field's typetree names (object mode would look
//! up another class); a field whose value is not of a type some version's
//! typetree gives it (an `int` field holding a string, a class field
//! holding an `int`), or an integer outside the widest type any version
//! gives it (`ChannelInfo`'s `UInt8`s, `StreamInfo`'s `UInt32`s). UnityPy's
//! arithmetic on such values is uncharacterised, and the bounds keep every
//! loop the handler runs proportional to its input. A message names a node's
//! type by at most [`NAME_BYTES`] bytes of it ([`type_name`]), so a refusal
//! allocates no more for a long one.
//!
//! **Work.** Each scan of a node's children counts one unit of decode work
//! for every name the scan may compare, counted before it, on the meter
//! [`Obj`] carries: [`Obj::get`] the children and the dict's entries, and
//! the test of whether object mode builds a class ([`builds`]) each child
//! against each field it looks for. So reading a class of N fields is work
//! in N, wherever the handler reads it.

use crate::limits::Meter;
use crate::typetree::{NodeId, NodeList};
use crate::{Map, Parsed, PrimArray, RawStr, Value};

use self::error::ViewError as MeshError;
mod error;
pub use error::ViewError;

/// The most bytes of a node's type a [`ViewError::Malformed`] message holds
/// (module docs).
pub const NAME_BYTES: usize = 64;

/// `ty` for a message: its bytes as `escape_ascii` escapes them, at most
/// [`NAME_BYTES`] of output, cut between two escapes and then followed by
/// `…`.
struct TypeName<'a>(&'a [u8]);
fn type_name(ty: &[u8]) -> TypeName<'_> {
    TypeName(ty)
}
impl std::fmt::Display for TypeName<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        let mut written = 0;
        for &b in self.0 {
            let escaped = b.escape_ascii();
            if written + escaped.len() > NAME_BYTES {
                return out.write_str("…");
            }
            written += escaped.len();
            for c in escaped {
                out.write_char(char::from(c))?;
            }
        }
        Ok(())
    }
}
struct TypeNames(&'static [&'static str]);
impl std::fmt::Display for TypeNames {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, ty) in self.0.iter().enumerate() {
            if i > 0 {
                out.write_str(" or ")?;
            }
            out.write_str(ty)?;
        }
        Ok(())
    }
}

/// Counts `n` units of work on `meter` (module docs).
fn count(meter: &Meter, n: usize) -> Result<(), MeshError> {
    meter.count_decode(u64::try_from(n).unwrap_or(u64::MAX))?;
    Ok(())
}

pub use classes::*;
mod classes;

/// Whether UnityPyBoost's `clean_name(name)`
/// (`UnityPyBoost/TypeTreeHelper.cpp:28-65`) is `target`, a plain
/// identifier: a leading `(int&)` and one trailing `?` dropped, and each of
/// ` .:-[]` read as `_` (the `pass`/`from` and leading-digit rules cannot
/// make any target here).
fn clean_name_is(name: &[u8], target: &str) -> bool {
    let name = name.strip_prefix(b"(int&)").unwrap_or(name);
    let name = name.strip_suffix(b"?").unwrap_or(name);
    let target = target.as_bytes();
    name.len() == target.len()
        && name.iter().zip(target).all(|(&a, &b)| {
            let a = if b" .:-[]".contains(&a) { b'_' } else { a };
            a == b
        })
}

/// Whether object mode builds `node` as `class` rather than an
/// `UnknownObject` (`UnityPyBoost/TypeTreeHelper.cpp:501-574`): every
/// required field among its children's clean names, and, for a slotted
/// class, none that is not a field. The comparisons are counted on `meter`
/// first: each child against each required field, and, for a slotted
/// class, against every field.
fn builds(list: &NodeList, node: NodeId, class: &Class, meter: &Meter) -> Result<bool, MeshError> {
    let kids = list.children(node);
    let fields = class.required.len().saturating_add(class.optional.len());
    let per_child = class
        .required
        .len()
        .saturating_add(if class.slotted { fields } else { 0 });
    count(meter, kids.len().saturating_mul(per_child))?;
    let has = |f: &str| kids.iter().any(|&c| clean_name_is(list.name(c), f));
    let is_field = |c: NodeId| {
        class
            .required
            .iter()
            .chain(class.optional)
            .any(|f| clean_name_is(list.name(c), f))
    };
    Ok(class.required.iter().all(|f| has(f))
        && (!class.slotted || kids.iter().all(|&c| is_field(c))))
}

/// One object, as object mode builds it from a class node (module docs),
/// with the meter its reads count their work on.
#[derive(Clone, Copy, Debug)]
pub struct Obj<'a> {
    list: &'a NodeList,
    node: NodeId,
    map: &'a Map,
    built: bool,
    meter: &'a Meter,
}

const _: () = crate::send_sync::<Obj<'static>>();

impl<'a> Obj<'a> {
    /// The root object of `parsed`, a node of `class`, its reads counted on
    /// `meter`.
    pub fn root(
        parsed: &'a Parsed,
        class: &'static Class,
        meter: &'a Meter,
    ) -> Result<Self, MeshError> {
        let list = parsed.lists().first().ok_or_else(|| {
            MeshError::malformed(format_args!("{}", "a value without a tree"), meter)
        })?;
        Self::new(
            list,
            list.root(),
            parsed.value(),
            class,
            "the object",
            meter,
        )
    }

    /// `node`'s object, `value` its dict.
    fn new(
        list: &'a NodeList,
        node: NodeId,
        value: &'a Value,
        class: &'static Class,
        field: &str,
        meter: &'a Meter,
    ) -> Result<Self, MeshError> {
        let ty = list.type_name(node);
        if !class.types.iter().any(|t| t.as_bytes() == ty) {
            return Err(MeshError::malformed(
                format_args!(
                    "{field} has type {}, not {}",
                    type_name(ty),
                    TypeNames(class.types)
                ),
                meter,
            ));
        }
        let Value::Dict(map) = value else {
            return Err(MeshError::malformed(
                format_args!("{field} is not a class"),
                meter,
            ));
        };
        if map.list() != 0 {
            return Err(MeshError::malformed(
                format_args!("{field} is read through a referenced type"),
                meter,
            ));
        }
        Ok(Self {
            list,
            node,
            map,
            built: builds(list, node, class, meter)?,
            meter,
        })
    }

    /// Whether object mode built the class, not an `UnknownObject`.
    #[must_use]
    pub fn built(&self) -> bool {
        self.built
    }

    /// The child whose clean name is `name`: the last, as object mode's
    /// dict keeps the last value of a name.
    fn child(&self, name: &str) -> Option<NodeId> {
        self.list
            .children(self.node)
            .iter()
            .rev()
            .find(|&&c| clean_name_is(self.list.name(c), name))
            .copied()
    }

    /// The attribute `name` (module docs): its value; `None`, the field's
    /// default, on a class that was built; `AttributeError` on an
    /// `UnknownObject`. Its scans, of the children and of the dict, are
    /// counted first, a unit a name.
    pub fn get(&self, name: &'static str) -> Result<Option<(NodeId, &'a Value)>, MeshError> {
        let scanned = self
            .list
            .children(self.node)
            .len()
            .saturating_add(self.map.entries().len());
        count(self.meter, scanned)?;
        let found = self.child(name).and_then(|c| {
            let raw = self.list.name(c);
            self.map
                .entries()
                .iter()
                .find(|(k, _)| self.list.name(*k) == raw)
                .map(|(_, v)| (c, v))
        });
        match found {
            Some(hit) => Ok(Some(hit)),
            None if self.built => Ok(None),
            None => Err(MeshError::NoAttribute(name)),
        }
    }

    /// The attribute `name`, an integer from 0 to `max` (a Python `bool`
    /// reads as its `int`, as Python's arithmetic takes it).
    pub fn uint(&self, name: &'static str, max: u64) -> Result<Option<u64>, MeshError> {
        let Some((_, v)) = self.get(name)? else {
            return Ok(None);
        };
        let n = match v {
            Value::Bool(b) => Some(u64::from(*b)),
            Value::Int(i) => u64::try_from(*i).ok(),
            Value::UInt(u) => Some(*u),
            _ => {
                return Err(MeshError::malformed(
                    format_args!("{name} is not an integer"),
                    self.meter,
                ));
            }
        };
        match n {
            Some(n) if n <= max => Ok(Some(n)),
            _ => Err(MeshError::malformed(
                format_args!("{name} is out of its range"),
                self.meter,
            )),
        }
    }

    /// The attribute `name`, a 32-bit signed integer (a `bool` as its
    /// `int`).
    pub fn int32(&self, name: &'static str) -> Result<Option<i64>, MeshError> {
        let Some((_, v)) = self.get(name)? else {
            return Ok(None);
        };
        let n = match v {
            Value::Bool(b) => i64::from(*b),
            Value::Int(i) if i32::try_from(*i).is_ok() => *i,
            Value::Int(_) | Value::UInt(_) => {
                return Err(MeshError::malformed(
                    format_args!("{name} is out of its range"),
                    self.meter,
                ));
            }
            _ => {
                return Err(MeshError::malformed(
                    format_args!("{name} is not an integer"),
                    self.meter,
                ));
            }
        };
        Ok(Some(n))
    }

    /// The attribute `name`, a `float` (a `float` or `double` leaf, read as
    /// Python reads it: a `float`'s bits widened as `struct`'s `f` widens
    /// them, [`widen`]).
    pub fn float(&self, name: &'static str) -> Result<Option<f64>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, Value::F32(bits))) => Ok(Some(widen(*bits))),
            Some((_, Value::F64(bits))) => Ok(Some(f64::from_bits(*bits))),
            Some(_) => Err(MeshError::malformed(
                format_args!("{name} is not a float"),
                self.meter,
            )),
        }
    }

    /// The attribute `name`, `bytes` (`TypelessData`).
    pub fn bytes(&self, name: &'static str) -> Result<Option<&'a [u8]>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, Value::Bytes(b))) => Ok(Some(b)),
            Some(_) => Err(MeshError::malformed(
                format_args!("{name} is not bytes"),
                self.meter,
            )),
        }
    }

    /// The attribute `name`, a `str`.
    pub fn string(&self, name: &'static str) -> Result<Option<&'a RawStr>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, Value::Str(s))) => Ok(Some(s)),
            Some(_) => Err(MeshError::malformed(
                format_args!("{name} is not a string"),
                self.meter,
            )),
        }
    }

    /// The attribute `name`, an object of `class`.
    pub fn class(
        &self,
        name: &'static str,
        class: &'static Class,
    ) -> Result<Option<Obj<'a>>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((node, v)) => Self::new(self.list, node, v, class, name, self.meter).map(Some),
        }
    }

    /// The attribute `name`, a list of objects of `class`.
    pub fn list(
        &self,
        name: &'static str,
        class: &'static Class,
    ) -> Result<Option<List<'a>>, MeshError> {
        let Some((node, v)) = self.get(name)? else {
            return Ok(None);
        };
        let items: &'a [Value] = match v {
            Value::List(items) => items,
            Value::Tuple(items) => items,
            Value::Prims(p) if p.is_empty() => &[],
            _ => {
                return Err(MeshError::malformed(
                    format_args!("{name} is not a list of classes"),
                    self.meter,
                ));
            }
        };
        // UnityPy TypeTreeHelper.cpp:812-874: a vector's element is its
        // `Array` child's second child.
        let elem = match self.list.children(node) {
            [array, ..] => match self.list.children(*array) {
                [_, elem] => Some(*elem),
                _ => None,
            },
            [] => None,
        };
        let Some(elem) = elem else {
            return Err(MeshError::malformed(
                format_args!("{name} is not a vector"),
                self.meter,
            ));
        };
        Ok(Some(List {
            nodes: self.list,
            elem,
            items,
            class,
            name,
            built: builds(self.list, elem, class, self.meter)?,
            meter: self.meter,
        }))
    }

    /// The attribute `name`, a list of integers.
    pub fn ints(&self, name: &'static str) -> Result<Option<Ints<'a>>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, v)) => Ints::of(v, name, self.meter).map(Some),
        }
    }
}

/// A list of objects of one class (one element node, so one answer to
/// whether object mode builds them).
#[derive(Clone, Copy, Debug)]
pub struct List<'a> {
    nodes: &'a NodeList,
    elem: NodeId,
    items: &'a [Value],
    class: &'static Class,
    name: &'static str,
    built: bool,
    meter: &'a Meter,
}

const _: () = crate::send_sync::<List<'static>>();

impl<'a> List<'a> {
    /// The number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Element `i`, if there is one.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<Result<Obj<'a>, MeshError>> {
        let v = self.items.get(i)?;
        Some(self.element(v))
    }

    fn element(&self, v: &'a Value) -> Result<Obj<'a>, MeshError> {
        let ty = self.nodes.type_name(self.elem);
        if !self.class.types.iter().any(|t| t.as_bytes() == ty) {
            return Err(MeshError::malformed(
                format_args!("{}'s elements have type {}", self.name, type_name(ty)),
                self.meter,
            ));
        }
        let Value::Dict(map) = v else {
            return Err(MeshError::malformed(
                format_args!("{}'s elements are not classes", self.name),
                self.meter,
            ));
        };
        if map.list() != 0 {
            return Err(MeshError::malformed(
                format_args!("{} is read through a referenced type", self.name),
                self.meter,
            ));
        }
        Ok(Obj {
            list: self.nodes,
            node: self.elem,
            map,
            built: self.built,
            meter: self.meter,
        })
    }

    /// The elements in order.
    pub fn iter(&self) -> impl Iterator<Item = Result<Obj<'a>, MeshError>> + '_ {
        self.items.iter().map(|v| self.element(v))
    }
}

/// A list of integers: a primitive run, or a list or tuple of `int`s.
#[derive(Clone, Copy, Debug)]
pub enum Ints<'a> {
    /// A primitive run.
    Run(&'a PrimArray, &'a Meter),
    /// A list or tuple of values, each checked as it is read.
    Items(&'a [Value], &'static str, &'a Meter),
    /// `bytes` (`TypelessData`), which `bytes()` copies as it is.
    Raw(&'a [u8], &'a Meter),
}

const _: () = crate::send_sync::<Ints<'static>>();

impl<'a> Ints<'a> {
    fn meter(&self) -> &Meter {
        match self {
            Self::Run(_, m) | Self::Items(_, _, m) | Self::Raw(_, m) => m,
        }
    }

    fn of(v: &'a Value, name: &'static str, meter: &'a Meter) -> Result<Self, MeshError> {
        match v {
            Value::Prims(p) => match p {
                PrimArray::F32(_) | PrimArray::F64(_) if !p.is_empty() => Err(
                    MeshError::malformed(format_args!("{name} is not a list of integers"), meter),
                ),
                _ => Ok(Self::Run(p, meter)),
            },
            Value::List(items) => Ok(Self::Items(items, name, meter)),
            Value::Tuple(items) => Ok(Self::Items(items, name, meter)),
            Value::Bytes(b) => Ok(Self::Raw(b, meter)),
            _ => Err(MeshError::malformed(
                format_args!("{name} is not a list"),
                meter,
            )),
        }
    }

    /// The number of integers.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Run(p, _) => p.len(),
            Self::Items(items, _, _) => items.len(),
            Self::Raw(b, _) => b.len(),
        }
    }

    /// Whether there are no integers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Element `i` of `bytes(self)`: the value, which must be a byte
    /// (`ValueError` "bytes must be in range(0, 256)" otherwise; a `bool`
    /// is its `int`). `i` past the end reads as 0.
    pub fn byte(&self, i: usize) -> Result<u8, MeshError> {
        fn byte<T: TryInto<u8>>(v: Option<T>) -> Result<u8, MeshError> {
            v.map_or(Ok(0), |v| v.try_into().map_err(|_| MeshError::ByteRange))
        }
        match self {
            Self::Run(p, _) => match p {
                PrimArray::U8(b) => Ok(b.get(i).copied().unwrap_or(0)),
                PrimArray::Bool(v) => Ok(v.get(i).copied().map_or(0, u8::from)),
                PrimArray::I8(v) => byte(v.get(i).copied()),
                PrimArray::U16(v) => byte(v.get(i).copied()),
                PrimArray::I16(v) => byte(v.get(i).copied()),
                PrimArray::U32(v) => byte(v.get(i).copied()),
                PrimArray::I32(v) => byte(v.get(i).copied()),
                PrimArray::U64(v) => byte(v.get(i).copied()),
                PrimArray::I64(v) => byte(v.get(i).copied()),
                // Refused by `of` unless empty.
                PrimArray::F32(_) | PrimArray::F64(_) => Ok(0),
            },
            Self::Items(items, name, _) => match items.get(i) {
                None => Ok(0),
                Some(Value::Bool(b)) => Ok(u8::from(*b)),
                Some(Value::Int(v)) => byte(Some(*v)),
                Some(Value::UInt(v)) => byte(Some(*v)),
                Some(_) => Err(MeshError::malformed(
                    format_args!("{name} holds a value that is not an integer"),
                    self.meter(),
                )),
            },
            Self::Raw(b, _) => Ok(b.get(i).copied().unwrap_or(0)),
        }
    }

    /// Item `i` as `m_Data[i]` reads it (`helpers/PackedBitVector.py:56`):
    /// `None` past the end, where Python raises `IndexError`; a value that
    /// is no byte, which no typetree's `UInt8` holds, is `Malformed`.
    pub fn item(&self, i: usize) -> Result<Option<u8>, MeshError> {
        if i >= self.len() {
            return Ok(None);
        }
        match self.byte(i) {
            Ok(b) => Ok(Some(b)),
            Err(MeshError::ByteRange) => Err(MeshError::malformed(
                format_args!("{}", "m_Data holds a value past a byte"),
                self.meter(),
            )),
            Err(e) => Err(e),
        }
    }

    /// Whether Python holds it as `bytes`, whose index error reads "index
    /// out of range", not a list's.
    #[must_use]
    pub fn is_bytes(&self) -> bool {
        matches!(self, Self::Raw(_, _))
    }

    /// The bytes themselves, where they need no check (a `U8` run, or
    /// `bytes`).
    #[must_use]
    pub fn as_u8(&self) -> Option<&'a [u8]> {
        match self {
            Self::Run(PrimArray::U8(b), _) => Some(b),
            Self::Raw(b, _) => Some(b),
            _ => None,
        }
    }
}

fn widen(bits: u32) -> f64 {
    let value = f32::from_bits(bits);
    if value.is_nan() {
        f64::from_bits(
            (u64::from(bits >> 31) << 63)
                | 0x7ff0_0000_0000_0000
                | (u64::from((bits & 0x007f_ffff) | 0x0040_0000) << 29),
        )
    } else {
        f64::from(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{NAME_BYTES, type_name};

    #[test]
    fn a_type_name_is_cut_between_escapes() {
        assert_eq!(type_name(b"Mesh").to_string(), "Mesh");
        let exact = "x".repeat(NAME_BYTES);
        assert_eq!(type_name(exact.as_bytes()).to_string(), exact);
        let long = "y".repeat(1 << 20);
        assert_eq!(
            type_name(long.as_bytes()).to_string(),
            format!("{}…", &long[..NAME_BYTES])
        );
        // Each 0xFF escapes to four bytes, `\xff`: sixteen fit.
        let cut = type_name(&[0xFF; 100]).to_string();
        assert_eq!(cut, format!("{}…", "\\xff".repeat(NAME_BYTES / 4)));
        // An escape that would cross the bound is left out whole.
        let mut ty = vec![b'a'; NAME_BYTES - 2];
        ty.push(0xFF);
        assert_eq!(
            type_name(&ty).to_string(),
            format!("{}…", "a".repeat(NAME_BYTES - 2))
        );
    }
}
