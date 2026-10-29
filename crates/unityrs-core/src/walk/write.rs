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

use std::collections::HashMap;

use super::dict::{names_match, slot_count, steps};
use super::{RefTypes, WriteCtx, WriteError, WriteRules, py_size_field_is_zero};
use crate::budget::DecodeBuf;
use crate::error::{LimitError, LimitKind};
use crate::limits::{Charge, HASH_TABLE_SHARES, MeteredVec, heap_cost};
use crate::raw::RawStr;
use crate::typetree::{Kind, NodeId, Tree, TreeError};
use crate::value::{Map, Parsed, PrimArray, Value};

/// Writes `parsed` through `tree` (the object's own tree, or a `nodes=`
/// override: keys are then matched by name), appending to `out`, a buffer
/// on the object's input's meter that charges before it grows (a value read
/// with a negative length can name the same bytes many times, so the output
/// can be far larger than the value; plan section 3.15).
pub fn write(
    parsed: &Parsed,
    tree: &Tree,
    ctx: &WriteCtx<'_>,
    out: &mut DecodeBuf,
) -> Result<(), WriteError> {
    let base = out.len();
    let mut w = Writer {
        parsed,
        out,
        base,
        big: ctx.endian == crate::stream::Endian::Big,
        rules: ctx.rules,
        ref_types: ctx.ref_types,
        max_depth: ctx.limits.max_value_depth,
        limits: *ctx.limits,
        depth: 0,
        has_registry: false,
    };
    w.value(tree, tree.root(), Obj::V(parsed.value()))
}

/// The Python object a value is to UnityPy's writer, which duck-types
/// (W2): a value of the tree, or one Python's operations make from one (an
/// element of a `str`, `bytes` or a run, a dict's key, an attribute).
#[derive(Clone, Copy, Debug)]
enum Obj<'a> {
    V(&'a Value),
    Int(i128),
    Bool(bool),
    F32(u32),
    F64(u64),
    /// One character of a `str` (its surrogate-escaped bytes), or a key.
    Str(&'a [u8]),
    /// A bound method or a builtin: truthy, no attribute the writer reads.
    Method,
}

/// The Python type an object is, for `getattr`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PyType {
    Int,
    Bool,
    Float,
    Str,
    Bytes,
    List,
    Tuple,
    Dict,
    Method,
}

/// Python 3.11's non-dunder attributes of each type a value becomes, and
/// which are data (`int`, `float`) rather than methods (`dir(type)`, recorded
/// by `tools/fixtures/walk.py` into `py_attrs.json`).
const INT_ATTRS: &[(&str, &str)] = &[
    ("as_integer_ratio", "method"),
    ("bit_count", "method"),
    ("bit_length", "method"),
    ("conjugate", "method"),
    ("denominator", "int"),
    ("from_bytes", "method"),
    ("imag", "int"),
    ("numerator", "int"),
    ("real", "int"),
    ("to_bytes", "method"),
];
const FLOAT_ATTRS: &[(&str, &str)] = &[
    ("as_integer_ratio", "method"),
    ("conjugate", "method"),
    ("fromhex", "method"),
    ("hex", "method"),
    ("imag", "float"),
    ("is_integer", "method"),
    ("real", "float"),
];
const STR_ATTRS: &[(&str, &str)] = &[
    ("capitalize", "method"),
    ("casefold", "method"),
    ("center", "method"),
    ("count", "method"),
    ("encode", "method"),
    ("endswith", "method"),
    ("expandtabs", "method"),
    ("find", "method"),
    ("format", "method"),
    ("format_map", "method"),
    ("index", "method"),
    ("isalnum", "method"),
    ("isalpha", "method"),
    ("isascii", "method"),
    ("isdecimal", "method"),
    ("isdigit", "method"),
    ("isidentifier", "method"),
    ("islower", "method"),
    ("isnumeric", "method"),
    ("isprintable", "method"),
    ("isspace", "method"),
    ("istitle", "method"),
    ("isupper", "method"),
    ("join", "method"),
    ("ljust", "method"),
    ("lower", "method"),
    ("lstrip", "method"),
    ("maketrans", "method"),
    ("partition", "method"),
    ("removeprefix", "method"),
    ("removesuffix", "method"),
    ("replace", "method"),
    ("rfind", "method"),
    ("rindex", "method"),
    ("rjust", "method"),
    ("rpartition", "method"),
    ("rsplit", "method"),
    ("rstrip", "method"),
    ("split", "method"),
    ("splitlines", "method"),
    ("startswith", "method"),
    ("strip", "method"),
    ("swapcase", "method"),
    ("title", "method"),
    ("translate", "method"),
    ("upper", "method"),
    ("zfill", "method"),
];
const BYTES_ATTRS: &[(&str, &str)] = &[
    ("capitalize", "method"),
    ("center", "method"),
    ("count", "method"),
    ("decode", "method"),
    ("endswith", "method"),
    ("expandtabs", "method"),
    ("find", "method"),
    ("fromhex", "method"),
    ("hex", "method"),
    ("index", "method"),
    ("isalnum", "method"),
    ("isalpha", "method"),
    ("isascii", "method"),
    ("isdigit", "method"),
    ("islower", "method"),
    ("isspace", "method"),
    ("istitle", "method"),
    ("isupper", "method"),
    ("join", "method"),
    ("ljust", "method"),
    ("lower", "method"),
    ("lstrip", "method"),
    ("maketrans", "method"),
    ("partition", "method"),
    ("removeprefix", "method"),
    ("removesuffix", "method"),
    ("replace", "method"),
    ("rfind", "method"),
    ("rindex", "method"),
    ("rjust", "method"),
    ("rpartition", "method"),
    ("rsplit", "method"),
    ("rstrip", "method"),
    ("split", "method"),
    ("splitlines", "method"),
    ("startswith", "method"),
    ("strip", "method"),
    ("swapcase", "method"),
    ("title", "method"),
    ("translate", "method"),
    ("upper", "method"),
    ("zfill", "method"),
];
const LIST_ATTRS: &[(&str, &str)] = &[
    ("append", "method"),
    ("clear", "method"),
    ("copy", "method"),
    ("count", "method"),
    ("extend", "method"),
    ("index", "method"),
    ("insert", "method"),
    ("pop", "method"),
    ("remove", "method"),
    ("reverse", "method"),
    ("sort", "method"),
];
const TUPLE_ATTRS: &[(&str, &str)] = &[("count", "method"), ("index", "method")];

/// The writer's table of a Python type's non-dunder attributes and their
/// kinds (`"method"`, `"int"`, `"float"`), by the type's name; empty for a
/// type it does not model. Public for the fixture test that checks it
/// against Python 3.11's `dir()`.
#[doc(hidden)]
#[must_use]
pub fn python_attributes(type_name: &str) -> &'static [(&'static str, &'static str)] {
    match type_name {
        "int" | "bool" => INT_ATTRS,
        "float" => FLOAT_ATTRS,
        "str" => STR_ATTRS,
        "bytes" => BYTES_ATTRS,
        "list" => LIST_ATTRS,
        "tuple" => TUPLE_ATTRS,
        _ => &[],
    }
}

/// A clean name longer than every attribute the writer models.
const CLEAN_MAX: usize = 32;

/// UnityPyBoost's `clean_name` (`UnityPyBoost/TypeTreeHelper.cpp:28-65`;
/// the C++ node is the one UnityPy builds when the extension loads), into a
/// fixed buffer: `None` when the name cleans to more than [`CLEAN_MAX`]
/// bytes, which no modelled attribute is.
fn clean_name(name: &[u8], buf: &mut [u8; CLEAN_MAX]) -> Option<usize> {
    // `:1218-1220`: the name is a C string (`PyUnicode_AsUTF8`), so it ends
    // at its first NUL, and so does the cleaned one.
    let mut name = name.split(|b| *b == 0).next().unwrap_or_default();
    // `:38-41`: the `(int&)` prefix.
    if let Some(rest) = name.strip_prefix(b"(int&)") {
        name = rest;
    }
    // `:44-47`: one trailing `?`.
    if let Some(rest) = name.strip_suffix(b"?") {
        name = rest;
    }
    // `:58-62`: a leading digit takes an `x` (`isdigit`, ASCII in the C
    // locale); such a name is never `pass` or `from`.
    let digit = name.first().is_some_and(u8::is_ascii_digit);
    let pass_from = matches!(name, b"pass" | b"from");
    let len = name.len() + usize::from(digit) + usize::from(pass_from);
    let out = buf.get_mut(..len)?;
    let (head, rest) = out.split_at_mut(usize::from(digit));
    head.fill(b'x');
    for (o, b) in rest.iter_mut().zip(name.iter().chain(b"_".iter())) {
        // `:50`: `[ \.:\-\[\]]` becomes `_`.
        *o = if matches!(b, b' ' | b'.' | b':' | b'-' | b'[' | b']') {
            b'_'
        } else {
            *b
        };
    }
    Some(len)
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
    limits: crate::limits::Limits,
    max_depth: u32,
    depth: u32,
    has_registry: bool,
}

/// Where each child of a class or `ReferencedObject` node finds its entry in
/// a dict, `value[child.m_Name]`, in steps linear in the children, as a
/// Python dict finds it. A cursor walks the entries in step with the
/// children (a read sets them in child order). A dict read through this
/// node also keeps a per-node table from a name's slot to its position, for
/// a child whose name an earlier child took. Any other miss (a dict keyed by
/// another list: a `nodes=` override, a dict a repeated name hands a
/// same-named sibling, a caller's map) finds the name in a table from each
/// name to its first entry, built on the first miss, once per dict. Both
/// tables are charged to the value's meter.
struct Lookup<'a> {
    map: &'a Map,
    /// Keyed by this node's children: read through it.
    native: bool,
    /// Slot to position, for a node whose children repeat a name.
    found: MeteredVec<u32>,
    cursor: usize,
    by_name: Option<(HashMap<&'a [u8], u32>, Charge)>,
}

const UNSET: u32 = u32::MAX;

impl<'a> Lookup<'a> {
    fn new(
        parsed: &'a Parsed,
        tree: &Tree,
        node: NodeId,
        map: &'a Map,
    ) -> Result<Self, WriteError> {
        let kids = tree.list().children(node);
        // (A dict read through a same-named sibling, which a repeated name
        // hands this node, is keyed by that sibling's children.)
        let native = usize::try_from(map.list())
            .ok()
            .and_then(|i| parsed.lists().get(i))
            .is_some_and(|l| Arc::ptr_eq(l, tree.list()))
            && map
                .entries()
                .first()
                .is_none_or(|(k, _)| kids.binary_search_by_key(&k.0, |c| c.0).is_ok());
        let slots = if native { slot_count(tree, node) } else { 0 };
        let mut found: MeteredVec<u32> = MeteredVec::new(parsed.charge().meter());
        if slots < kids.len() {
            found.reserve_exact(slots).map_err(WriteError::Charge)?;
            for _ in 0..slots {
                found.push(UNSET).map_err(WriteError::Charge)?;
            }
        }
        Ok(Self {
            map,
            native,
            found,
            cursor: 0,
            by_name: None,
        })
    }

    /// `child`'s entry.
    fn get(
        &mut self,
        parsed: &'a Parsed,
        tree: &Tree,
        child: NodeId,
    ) -> Result<Option<&'a Value>, WriteError> {
        let entries = self.map.entries();
        let name = tree.list().name(child);
        steps(1);
        let at_cursor = entries.get(self.cursor).is_some_and(|(k, _)| {
            if self.native {
                Some(*k) == tree.info(child).map(|i| i.key)
            } else {
                parsed.key_name(self.map, *k) == name
            }
        });
        let slot = tree
            .info(child)
            .filter(|_| self.native)
            .map(|i| i.slot as usize);
        let known = slot
            .and_then(|s| self.found.as_slice().get(s).copied())
            .filter(|p| *p != UNSET)
            .map(|p| p as usize);
        let pos = match (known, at_cursor) {
            (Some(p), _) => Some(p),
            (None, true) => Some(self.cursor),
            (None, false) => self.by_name(parsed)?.get(name).map(|p| *p as usize),
        };
        if let (Some(p), Some(f)) = (pos, slot.and_then(|s| self.found.as_mut_slice().get_mut(s))) {
            *f = u32::try_from(p).unwrap_or(UNSET);
        }
        if let Some(p) = pos
            && p >= self.cursor
        {
            self.cursor = p + 1;
        }
        Ok(pos.and_then(|p| entries.get(p)).map(|(_, v)| v))
    }

    /// Each name's first entry: built once, charged before it allocates
    /// ([`HASH_TABLE_SHARES`] buckets a name, and a table's fixed part).
    fn by_name(&mut self, parsed: &'a Parsed) -> Result<&HashMap<&'a [u8], u32>, WriteError> {
        let built = if let Some(b) = self.by_name.take() {
            b
        } else {
            let n = self.map.entries().len();
            let bucket = (size_of::<(&[u8], u32)>() + 1) as u64;
            let bytes = (n.max(2) as u64)
                .saturating_mul(HASH_TABLE_SHARES * bucket)
                .saturating_add(16);
            let charge = parsed
                .charge()
                .meter()
                .charge(heap_cost(bytes))
                .map_err(WriteError::Charge)?;
            let mut first = HashMap::with_capacity(n);
            for (i, (name, _)) in parsed.entries(self.map).enumerate() {
                steps(1);
                first
                    .entry(name)
                    .or_insert(u32::try_from(i).unwrap_or(UNSET));
            }
            (first, charge)
        };
        Ok(&self.by_name.insert(built).0)
    }
}

/// The characters of a surrogate-escaped `str`: each valid UTF-8 character,
/// and each byte that is not part of one (one escape character each).
fn chars(s: &[u8]) -> impl Iterator<Item = &[u8]> {
    s.utf8_chunks().flat_map(|c| {
        let valid = c.valid();
        let v = valid.char_indices().map(move |(i, ch)| {
            valid
                .as_bytes()
                .get(i..i + ch.len_utf8())
                .unwrap_or_default()
        });
        let invalid = c.invalid();
        v.chain((0..invalid.len()).map(move |i| invalid.get(i..=i).unwrap_or_default()))
    })
}

fn narrow(x: f64) -> Option<u32> {
    // `PyFloat_Pack4`: `(float)x`, an overflow refused; a NaN's quiet bit
    // set and payload kept, as x86-64's `cvtsd2ss`.
    if x.is_nan() {
        let b = x.to_bits();
        let sign = u32::try_from(b >> 63).unwrap_or(0) << 31;
        let mant = u32::try_from((b >> 29) & 0x003f_ffff).unwrap_or(0);
        return Some(sign | 0x7fc0_0000 | mant);
    }
    #[allow(clippy::cast_possible_truncation)]
    let y = x as f32;
    if y.is_infinite() && !x.is_infinite() {
        return None;
    }
    Some(y.to_bits())
}

/// `(double)f` on x86-64: exact, a NaN's quiet bit set.
fn widen(bits: u32) -> f64 {
    let f = f32::from_bits(bits);
    if f.is_nan() {
        let sign = u64::from(bits >> 31) << 63;
        let mant = u64::from((bits & 0x007f_ffff) | 0x0040_0000) << 29;
        return f64::from_bits(sign | 0x7ff0_0000_0000_0000 | mant);
    }
    f64::from(f)
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

    fn py_type(o: Obj<'_>) -> PyType {
        match o {
            Obj::V(Value::Bool(_)) | Obj::Bool(_) => PyType::Bool,
            Obj::V(Value::Int(_) | Value::UInt(_)) | Obj::Int(_) => PyType::Int,
            Obj::V(Value::F32(_) | Value::F64(_)) | Obj::F32(_) | Obj::F64(_) => PyType::Float,
            Obj::V(Value::Str(_)) | Obj::Str(_) => PyType::Str,
            Obj::V(Value::Bytes(_)) => PyType::Bytes,
            Obj::V(Value::Prims(_) | Value::List(_)) => PyType::List,
            Obj::V(Value::Tuple(_)) => PyType::Tuple,
            Obj::V(Value::Dict(_)) => PyType::Dict,
            Obj::Method => PyType::Method,
        }
    }

    /// `bool(o)`.
    fn truthy(o: Obj<'_>) -> bool {
        match o {
            Obj::V(v) => match v {
                Value::F32(b) => widen(*b) != 0.0,
                Value::F64(b) => f64::from_bits(*b) != 0.0,
                Value::Bool(b) => *b,
                Value::Int(i) => *i != 0,
                Value::UInt(_) => true,
                Value::Str(s) => !s.is_empty(),
                Value::Bytes(b) => !b.is_empty(),
                Value::Prims(p) => !p.is_empty(),
                Value::List(l) => !l.is_empty(),
                Value::Tuple(t) => !t.is_empty(),
                Value::Dict(m) => !m.is_empty(),
            },
            Obj::Int(i) => i != 0,
            Obj::Bool(b) => b,
            Obj::F32(b) => widen(b) != 0.0,
            Obj::F64(b) => f64::from_bits(b) != 0.0,
            Obj::Str(s) => !s.is_empty(),
            Obj::Method => true,
        }
    }

    /// The integer `struct.pack` takes (`int`, `bool`).
    fn as_int(o: Obj<'_>) -> Option<i128> {
        match o {
            Obj::V(Value::Bool(b)) => Some(i128::from(*b)),
            Obj::Bool(b) => Some(i128::from(b)),
            Obj::V(Value::Int(i)) => Some(i128::from(*i)),
            Obj::V(Value::UInt(u)) => Some(i128::from(*u)),
            Obj::Int(i) => Some(i),
            _ => None,
        }
    }

    /// The `float` `struct.pack('d')` takes: `float(o)` of an `int`, `bool`
    /// or `float`.
    #[allow(clippy::cast_precision_loss)]
    fn as_double(o: Obj<'_>) -> Option<f64> {
        match o {
            Obj::V(Value::F32(b)) => Some(widen(*b)),
            Obj::F32(b) => Some(widen(b)),
            Obj::V(Value::F64(b)) => Some(f64::from_bits(*b)),
            Obj::F64(b) => Some(f64::from_bits(b)),
            _ => Self::as_int(o).map(|i| i as f64),
        }
    }

    /// The bytes of a `str` (`value.encode("utf8", "surrogateescape")`).
    fn as_str(o: Obj<'_>) -> Option<&[u8]> {
        match o {
            Obj::V(Value::Str(s)) => Some(s.as_bytes()),
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `len(o)`.
    fn len(o: Obj<'_>) -> Option<usize> {
        match o {
            Obj::V(Value::Str(s)) => Some(chars(s.as_bytes()).count()),
            Obj::Str(s) => Some(chars(s).count()),
            Obj::V(Value::Bytes(b)) => Some(b.len()),
            Obj::V(Value::Prims(p)) => Some(p.len()),
            Obj::V(Value::List(l)) => Some(l.len()),
            Obj::V(Value::Tuple(t)) => Some(t.len()),
            Obj::V(Value::Dict(m)) => Some(m.len()),
            _ => None,
        }
    }

    fn prim_item(p: &PrimArray, i: usize) -> Option<Obj<'_>> {
        Some(match p {
            PrimArray::Bool(v) => Obj::Bool(*v.get(i)?),
            PrimArray::U8(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::I8(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::U16(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::I16(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::U32(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::I32(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::U64(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::I64(v) => Obj::Int(i128::from(*v.get(i)?)),
            PrimArray::F32(v) => Obj::F32(*v.get(i)?),
            PrimArray::F64(v) => Obj::F64(*v.get(i)?),
        })
    }

    /// `o[i]` for a sequence (a dict's keys are strings, so `d[0]` fails).
    fn item(o: Obj<'_>, i: usize) -> Option<Obj<'_>> {
        match o {
            Obj::V(Value::Str(s)) => chars(s.as_bytes()).nth(i).map(Obj::Str),
            Obj::Str(s) => chars(s).nth(i).map(Obj::Str),
            Obj::V(Value::Bytes(b)) => b.get(i).map(|x| Obj::Int(i128::from(*x))),
            Obj::V(Value::Prims(p)) => Self::prim_item(p, i),
            Obj::V(Value::List(l)) => l.get(i).map(Obj::V),
            Obj::V(Value::Tuple(t)) => t.get(i).map(Obj::V),
            _ => None,
        }
    }

    /// `getattr(o, name)` for a non-dict value (W2), `name` cleaned.
    fn getattr<'o>(o: Obj<'o>, name: &[u8]) -> Result<Option<Obj<'o>>, ()> {
        if name.len() >= 4 && name.starts_with(b"__") && name.ends_with(b"__") {
            // Every Python object has dunder attributes; the writer does not
            // model them (plan 3.11, W2: the names its matrix uses).
            return Err(());
        }
        let ty = Self::py_type(o);
        let table = match ty {
            PyType::Int | PyType::Bool => INT_ATTRS,
            PyType::Float => FLOAT_ATTRS,
            PyType::Str => STR_ATTRS,
            PyType::Bytes => BYTES_ATTRS,
            PyType::List => LIST_ATTRS,
            PyType::Tuple => TUPLE_ATTRS,
            PyType::Dict | PyType::Method => &[],
        };
        let Some((attr, kind)) = table.iter().find(|(a, _)| a.as_bytes() == name) else {
            return Ok(None);
        };
        Ok(Some(match (*kind, *attr) {
            ("method", _) => Obj::Method,
            ("int", "real" | "numerator") => Obj::Int(Self::as_int(o).unwrap_or(0)),
            ("int", "denominator") => Obj::Int(1),
            ("int", _) => Obj::Int(0),
            ("float", "real") => match o {
                Obj::V(Value::F32(b)) => Obj::F32(*b),
                Obj::F32(b) => Obj::F32(b),
                Obj::V(Value::F64(b)) => Obj::F64(*b),
                Obj::F64(b) => Obj::F64(b),
                _ => Obj::F64(0),
            },
            _ => Obj::F64(0),
        }))
    }

    /// `d[name]` on a dict value, by key string.
    fn field(&self, m: &'a Map, name: &[u8]) -> Option<&'a Value> {
        self.parsed.field(m, name)
    }

    fn value(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        self.frame(|w| w.value_inner(tree, node, o))
    }

    /// One frame UnityPy's `write_value` pushes: a `write_value` call, or the
    /// list comprehension that writes an array's elements (`:435`). Each
    /// counts one level against `max_value_depth`, as the pure-Python reader
    /// counts its own (plan 3.10, "Bounds"), so every value whose write
    /// raises `RecursionError` in UnityPy (past Python's 1,000 frames) is
    /// refused here first.
    fn frame(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), WriteError>,
    ) -> Result<(), WriteError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.max_depth {
            let e = WriteError::Limit(LimitError {
                kind: LimitKind::ValueDepth,
                value: u64::from(self.depth),
                max: u64::from(self.max_depth),
            });
            self.depth = self.depth.saturating_sub(1);
            return Err(e);
        }
        let r = f(self);
        self.depth = self.depth.saturating_sub(1);
        r
    }

    /// `write_value` (`TypeTreeHelper.py:407-458`).
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
            self.put(s)?;
            self.align()?;
        } else if kind == Kind::Bytes {
            // `write_byte_array` (`:110-112`), W4.
            let Obj::V(Value::Bytes(b)) = o else {
                return Err(Self::mismatch(tree, node, "bytes"));
            };
            self.write_int(b.len(), tree, node)?;
            self.put(b)?;
        } else if kind == Kind::Pair {
            // `:419-421`: `value[0]`, `value[1]` through the first two children.
            for i in 0..2 {
                let item = Self::item(o, i)
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
            if mirror_run {
                w.value_unaligned(tree, elem, item)
            } else {
                w.value(tree, elem, item)
            }
        };
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
        let kind = tree.py_kind(node);
        if kind.prim_size().is_some() {
            return self.prim(tree, node, kind, o);
        }
        let kids = tree.list().children(node);
        for i in 0..2 {
            let item =
                Self::item(o, i).ok_or_else(|| Self::mismatch(tree, node, "a sequence of two"))?;
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
        self.put(bytes)
    }

    /// The class branch (`:437-455`), W6: `value[child.m_Name]` for a dict,
    /// `getattr(value, child._clean_name)` otherwise; the registry scope as
    /// on read.
    fn class(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        if let Obj::V(Value::Dict(m)) = o {
            return self.class_dict(tree, node, m);
        }
        let outer = self.has_registry;
        let kids = tree.list().children(node);
        for c in kids {
            if tree.py_kind(*c) == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    continue;
                }
                self.has_registry = true;
            }
            let name = tree.list().name(*c);
            let item = if let Obj::V(Value::Dict(m)) = o {
                Obj::V(
                    self.field(m, name)
                        .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?,
                )
            } else {
                let mut buf = [0_u8; CLEAN_MAX];
                let clean = clean_name(name, &mut buf).and_then(|n| buf.get(..n));
                let attr = match clean {
                    Some(clean) => Self::getattr(o, clean),
                    None => Ok(None),
                };
                match attr {
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
                }
            };
            self.value(tree, *c, item)?;
        }
        self.has_registry = outer;
        Ok(())
    }

    /// The class branch on a dict (`:438-446`): `value[child.m_Name]` for
    /// each child, found by a [`Lookup`].
    fn class_dict(&mut self, tree: &Tree, node: NodeId, m: &'a Map) -> Result<(), WriteError> {
        let outer = self.has_registry;
        let mut find = Lookup::new(self.parsed, tree, node, m)?;
        for c in tree.list().children(node) {
            if tree.py_kind(*c) == Kind::ManagedReferencesRegistry {
                if self.has_registry {
                    continue;
                }
                self.has_registry = true;
            }
            let v = find
                .get(self.parsed, tree, *c)?
                .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
            self.value(tree, *c, Obj::V(v))?;
        }
        self.has_registry = outer;
        Ok(())
    }

    /// `ReferencedObject` (`:422-428`), W7: each child's entry found by a
    /// [`Lookup`].
    fn ref_object(&mut self, tree: &Tree, node: NodeId, o: Obj<'a>) -> Result<(), WriteError> {
        let mut find = None;
        for c in tree.list().children(node) {
            let Obj::V(Value::Dict(m)) = o else {
                return Err(Self::mismatch(tree, node, "dict"));
            };
            let find = match &mut find {
                Some(f) => f,
                None => find.insert(Lookup::new(self.parsed, tree, node, m)?),
            };
            if tree.py_kind(*c) != Kind::ReferencedObjectData {
                let v = find
                    .get(self.parsed, tree, *c)?
                    .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
                self.value(tree, *c, Obj::V(v))?;
                continue;
            }
            let ref_tree = match self.rules {
                WriteRules::UnityPy => self.ref_node_py(m)?,
                WriteRules::MirrorReader => match self.ref_node_boost(m)? {
                    // The reader skipped the data: so does the mirror.
                    None => continue,
                    Some(t) => Some(t),
                },
            };
            // `value[child.m_Name]`, then `write_value(..., None)` raises.
            let v = find
                .get(self.parsed, tree, *c)?
                .ok_or_else(|| WriteError::MissingKey(Self::name(tree, *c)))?;
            let Some(rt) = ref_tree else {
                return Err(Self::mismatch(tree, *c, "a ref type with a tree"));
            };
            self.value(&rt, rt.root(), Obj::V(v))?;
        }
        Ok(())
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
    fn names(&self, m: &'a Map) -> Result<(&'a Value, &'a Value, &'a Value), WriteError> {
        let key = |k: &'static [u8]| RawStr::new(Bytes::from_static(k));
        let t = match self.field(m, b"type") {
            None => return Err(WriteError::MissingKey(key(b"type"))),
            Some(Value::Dict(t)) => t,
            Some(_) => {
                return Err(WriteError::TypeMismatch {
                    node: key(b"type"),
                    expected: "a dict or an object with class, ns and asm",
                });
            }
        };
        let get = |k: &'static [u8]| {
            self.field(t, k)
                .ok_or_else(|| WriteError::MissingKey(key(k)))
        };
        Ok((get(b"class")?, get(b"ns")?, get(b"asm")?))
    }

    /// The pure-Python `get_ref_type_node` (`TypeTreeHelper.py:88-109`):
    /// `None` for an empty class; the writer then fails on it.
    fn ref_node_py(&self, m: &'a Map) -> Result<Option<Arc<Tree>>, WriteError> {
        let (cls, ns, asm) = self.names(m)?;
        let list = match self.ref_types {
            RefTypes::List(l) if !l.is_empty() => l,
            _ => return Err(WriteError::NoRefTypes),
        };
        if matches!(cls, Value::Str(s) if s.as_bytes().is_empty()) {
            return Ok(None);
        }
        let r = list
            .iter()
            .find(|r| names_match(r, cls, ns, asm))
            .ok_or(WriteError::RefTypeNotFound)?;
        r.tree(&self.limits).transpose().map_err(Self::tree_error)
    }

    /// UnityPyBoost's resolution (R14), for [`WriteRules::MirrorReader`]:
    /// `None` where the reader skipped the data.
    fn ref_node_boost(&self, m: &'a Map) -> Result<Option<Arc<Tree>>, WriteError> {
        let RefTypes::List(list) = self.ref_types else {
            return Err(WriteError::NoRefTypes);
        };
        let (cls, ns, asm) = self.names(m)?;
        if py_size_field_is_zero(cls) {
            return Ok(None);
        }
        let r = list
            .iter()
            .find(|r| names_match(r, cls, ns, asm))
            .ok_or(WriteError::RefTypeNotFound)?;
        r.tree(&self.limits).transpose().map_err(Self::tree_error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;

    use super::super::dict::STEPS;
    use crate::budget::DecodeBuf;
    use crate::error::LimitKind;
    use crate::limits::{Limits, Meter};
    use crate::stream::Endian;
    use crate::typetree::{NodeList, NodeRow, Tree};
    use crate::walk::{ReadCtx, ReadOptions, RefTypes, WriteCtx, WriteRules, boost, python};

    const N: u32 = 1 << 16;

    /// Reads `rows` over `data` with both readers and writes each value
    /// back, counting the writer's map steps.
    fn write_steps(rows: Vec<NodeRow>, data: &[u8]) -> [(&'static str, u64); 2] {
        let tree = Tree::compile(
            Arc::new(NodeList::trusted(rows).unwrap()),
            &Limits::default(),
        )
        .unwrap();
        let data = Bytes::copy_from_slice(data);
        let options = ReadOptions::default();
        let meter = Meter::unlimited();
        let ctx = ReadCtx {
            endian: Endian::Little,
            ref_types: RefTypes::List(&[]),
            options: &options,
            meter: &meter,
        };
        let limits = Limits::default();
        let wctx = WriteCtx {
            endian: Endian::Little,
            ref_types: RefTypes::List(&[]),
            rules: WriteRules::UnityPy,
            limits: &limits,
        };
        ["boost", "python"].map(|reader| {
            let p = match reader {
                "boost" => boost::read(&data, &tree, &ctx),
                _ => python::read(&data, 0, &tree, &ctx),
            }
            .unwrap();
            let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
            STEPS.with(|s| s.set(0));
            super::write(&p, &tree, &wctx, &mut out).unwrap();
            (reader, STEPS.with(std::cell::Cell::get))
        })
    }

    fn ints(n: u32) -> Vec<u8> {
        (0..n).flat_map(u32::to_le_bytes).collect()
    }

    /// Claude B's review: a `ReferencedObject` of 2^16 children, each
    /// `value[child.m_Name]`, written in steps linear in the children.
    #[test]
    fn a_wide_referenced_object_writes_in_linear_map_steps() {
        let mut rows = vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "ReferencedObject", "o"),
        ];
        for i in 0..N {
            rows.push(NodeRow::new(2, "int", format!("c{i}").as_str()));
        }
        for (reader, steps) in write_steps(rows, &ints(N)) {
            assert!(
                steps <= 2 * u64::from(N),
                "{reader}: {steps} map steps for {N} children"
            );
        }
    }

    /// A dict a repeated name hands a same-named sibling is keyed by the
    /// other's children, here in the reverse order: each child misses the
    /// cursor, and is found by name in steps linear in the children.
    #[test]
    fn a_dict_keyed_by_another_node_writes_in_linear_map_steps() {
        let mut rows = vec![NodeRow::new(0, "Base", "Base"), NodeRow::new(1, "Foo", "a")];
        for i in 0..N {
            rows.push(NodeRow::new(2, "int", format!("c{i}").as_str()));
        }
        rows.push(NodeRow::new(1, "Bar", "a"));
        for i in (0..N).rev() {
            rows.push(NodeRow::new(2, "int", format!("c{i}").as_str()));
        }
        for (reader, steps) in write_steps(rows, &ints(2 * N)) {
            assert!(
                steps <= 4 * u64::from(N),
                "{reader}: {steps} map steps for {N} children"
            );
        }
    }
}
