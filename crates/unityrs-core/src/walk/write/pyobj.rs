//! The Python objects UnityPy's writer duck-types (W2): what a value is to
//! Python ([`Obj`]), the attributes of each type the writer models, and the
//! operations the writer applies to an object (`bool`, `len`, `o[i]`,
//! `getattr`, the numbers `struct.pack` takes).

use super::Writer;
use crate::value::{PrimArray, Value};

/// The Python object a value is to UnityPy's writer, which duck-types
/// (W2): a value of the tree, or one Python's operations make from one (an
/// element of a `str`, `bytes` or a run, a dict's key, an attribute).
#[derive(Clone, Copy, Debug)]
pub(super) enum Obj<'a> {
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
pub(super) const CLEAN_MAX: usize = 32;

/// UnityPyBoost's `clean_name` (`UnityPyBoost/TypeTreeHelper.cpp:28-65`;
/// the C++ node is the one UnityPy builds when the extension loads), into a
/// fixed buffer: `None` when the name cleans to more than [`CLEAN_MAX`]
/// bytes, which no modelled attribute is. It reads at most the name's first
/// `CLEAN_MAX + 8` bytes: a C string longer than that cleans to more than
/// [`CLEAN_MAX`] whatever follows (review round 6's sweep: a write that
/// failed at a child of a long name had searched all of it for a NUL).
pub(super) fn clean_name(name: &[u8], buf: &mut [u8; CLEAN_MAX]) -> Option<usize> {
    // `:1218-1220`: the name is a C string (`PyUnicode_AsUTF8`), so it ends
    // at its first NUL, and so does the cleaned one. The `(int&)` prefix,
    // a trailing `?` and the NUL are the most a clean drops.
    let window = name.get(..CLEAN_MAX + 8).unwrap_or(name);
    crate::walk::steps(window.len());
    let mut name = match window.iter().position(|b| *b == 0) {
        Some(end) => window.get(..end).unwrap_or_default(),
        None if window.len() < name.len() => return None,
        None => window,
    };
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

/// The characters of a surrogate-escaped `str`: each valid UTF-8 character,
/// and each byte that is not part of one (one escape character each).
pub(super) fn chars(s: &[u8]) -> impl Iterator<Item = &[u8]> {
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

pub(super) fn narrow(x: f64) -> Option<u32> {
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

impl Writer<'_, '_> {
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
    pub(super) fn truthy(o: Obj<'_>) -> bool {
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
    pub(super) fn as_int(o: Obj<'_>) -> Option<i128> {
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
    pub(super) fn as_double(o: Obj<'_>) -> Option<f64> {
        match o {
            Obj::V(Value::F32(b)) => Some(widen(*b)),
            Obj::F32(b) => Some(widen(b)),
            Obj::V(Value::F64(b)) => Some(f64::from_bits(*b)),
            Obj::F64(b) => Some(f64::from_bits(b)),
            _ => Self::as_int(o).map(|i| i as f64),
        }
    }

    /// The bytes of a `str` (`value.encode("utf8", "surrogateescape")`).
    pub(super) fn as_str(o: Obj<'_>) -> Option<&[u8]> {
        match o {
            Obj::V(Value::Str(s)) => Some(s.as_bytes()),
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `len(o)`.
    pub(super) fn len(o: Obj<'_>) -> Option<usize> {
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

    pub(super) fn prim_item(p: &PrimArray, i: usize) -> Option<Obj<'_>> {
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
    pub(super) fn item(o: Obj<'_>, i: usize) -> Option<Obj<'_>> {
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
    pub(super) fn getattr<'o>(o: Obj<'o>, name: &[u8]) -> Result<Option<Obj<'o>>, ()> {
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
}
