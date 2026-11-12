//! JSON as `CPython` 3.11's `json.dumps(value, indent=2)` writes it at its
//! other defaults (`docs/plans/extract.md` E24): `ensure_ascii`, so every
//! character outside `' '..='~'` is a `\uXXXX` escape (lowercase hex, a
//! character past U+FFFF as its surrogate pair) but `\b`, `\f`, `\n`, `\r`
//! and `\t`, which have short forms, as `"` and `\` do; `allow_nan`, so a
//! NaN is `NaN` and the infinities `Infinity` and `-Infinity`; a float as
//! its `repr`; the separators `,` and `: ` that `indent` implies; an empty
//! list `[]` and an empty object `{}`, each on its line.
//!
//! A value is a typetree value as UnityPy 1.25.2's `read_typetree()` gives
//! it ([`unityrs_core::Value`]): a dict as an object in its field order, a
//! pair or any list as an array, an `f32` widened exactly to the double, a
//! string decoded from its bytes as Python's `surrogateescape` decodes them
//! (each byte of an invalid UTF-8 sequence becomes the lone surrogate
//! `U+DC00 + byte`, written `\udcXX`). Bytes (`TypelessData`) are a value
//! `json.dumps` cannot write ([`Unwritable`]).
//!
//! Every write goes through a [`Sink`]: [`Count`] measures a document
//! before anything is allocated for it, so the caller charges the exact
//! size first, then writes into a buffer of that capacity. The value walk
//! keeps an explicit stack, one frame per open container, charged to the
//! caller's meter before it grows, so no value's depth costs the thread's
//! stack.

use std::fmt::Write as _;
use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::pyfloat::py_float_repr;
use unityrs_core::{Map, Parsed, PrimArray, Value};

/// Where the writer puts its bytes.
pub trait Sink {
    /// Appends `bytes`.
    fn put(&mut self, bytes: &[u8]);
}

/// A [`Sink`] that only counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count(pub u64);

impl Sink for Count {
    fn put(&mut self, bytes: &[u8]) {
        let n = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.0 = self.0.saturating_add(n);
    }
}

impl Sink for Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// A value `json.dumps` raises on: bytes (`TypelessData`), which E25 makes
/// the atlas JSON's skip, `json_unwritable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a bytes value, which json.dumps cannot write")]
pub struct Unwritable;

/// A newline and `level` levels of two spaces.
pub fn newline(level: usize, out: &mut impl Sink) {
    out.put(b"\n");
    for _ in 0..level {
        out.put(b"  ");
    }
}

/// Python's `repr` of a double, as `json.dumps` writes a float.
pub fn float(x: f64, out: &mut impl Sink) {
    out.put(float_repr(x).as_bytes());
}

/// Python's `repr(x)` for a float as `json.dumps` writes it: the core's
/// [`py_float_repr`] (the nearest of the shortest digits that read back,
/// ties to even), but JSON's `NaN`, `Infinity` and `-Infinity`.
#[must_use]
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-Infinity" } else { "Infinity" }.to_owned();
    }
    py_float_repr(x)
}

/// `bytes` decoded as Python's `surrogateescape` decodes them: each valid
/// UTF-8 character as itself (`Ok`), each byte of an invalid sequence as one
/// item of its own (`Err(byte)`), in order.
pub fn decode_escaped(bytes: &[u8], mut each: impl FnMut(Result<char, u8>)) {
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                s.chars().for_each(|c| each(Ok(c)));
                return;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                let (good, tail) = rest.split_at(valid);
                if let Ok(s) = std::str::from_utf8(good) {
                    for c in s.chars() {
                        each(Ok(c));
                    }
                }
                let bad = e.error_len().unwrap_or(tail.len()).min(tail.len());
                let (invalid, after) = tail.split_at(bad);
                for &b in invalid {
                    each(Err(b));
                }
                rest = after;
            }
        }
    }
}

/// A JSON string of `bytes`, decoded as [`decode_escaped`] decodes them and
/// escaped as `ensure_ascii` escapes (module docs).
pub fn string(bytes: &[u8], out: &mut impl Sink) {
    out.put(b"\"");
    decode_escaped(bytes, |item| match item {
        Ok(c) => char_escaped(c, out),
        Err(b) => unit(0xDC00 + u32::from(b), out),
    });
    out.put(b"\"");
}

fn char_escaped(c: char, out: &mut impl Sink) {
    match c {
        '"' => out.put(b"\\\""),
        '\\' => out.put(b"\\\\"),
        '\n' => out.put(b"\\n"),
        '\r' => out.put(b"\\r"),
        '\t' => out.put(b"\\t"),
        '\u{8}' => out.put(b"\\b"),
        '\u{c}' => out.put(b"\\f"),
        ' '..='~' => {
            let mut buf = [0u8; 4];
            out.put(c.encode_utf8(&mut buf).as_bytes());
        }
        _ => {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                unit(u32::from(*u), out);
            }
        }
    }
}

/// `\uXXXX`, lowercase.
fn unit(u: u32, out: &mut impl Sink) {
    let mut s = String::with_capacity(6);
    let _ = write!(s, "\\u{u:04x}");
    out.put(s.as_bytes());
}

/// A decimal integer.
pub fn int(i: i128, out: &mut impl Sink) {
    out.put(i.to_string().as_bytes());
}

fn f32_value(bits: u32) -> f64 {
    f64::from(f32::from_bits(bits))
}

/// One element of a primitive run.
fn prim(arr: &PrimArray, i: usize, out: &mut impl Sink) {
    match arr {
        PrimArray::Bool(v) => {
            if let Some(&b) = v.get(i) {
                out.put(if b { b"true" } else { b"false" });
            }
        }
        PrimArray::U8(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::I8(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::U16(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::I16(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::U32(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::I32(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::U64(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::I64(v) => v.get(i).into_iter().for_each(|&x| int(x.into(), out)),
        PrimArray::F32(v) => v.get(i).into_iter().for_each(|&x| float(f32_value(x), out)),
        PrimArray::F64(v) => v
            .get(i)
            .into_iter()
            .for_each(|&x| float(f64::from_bits(x), out)),
    }
}

/// An open container's elements.
enum Items<'a> {
    Seq(&'a [Value]),
    Dict(&'a Map),
    Prims(&'a PrimArray),
}

impl Items<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Seq(v) => v.len(),
            Self::Dict(m) => m.len(),
            Self::Prims(p) => p.len(),
        }
    }
}

/// An open container: its elements, the next one, its level.
struct Frame<'a> {
    items: Items<'a>,
    next: usize,
    level: usize,
}

/// What a value opens: a container with elements, or nothing.
fn open(value: &Value) -> Option<Items<'_>> {
    let items = match value {
        Value::List(v) => Items::Seq(v),
        Value::Tuple(v) => Items::Seq(v),
        Value::Dict(m) => Items::Dict(m),
        Value::Prims(p) => Items::Prims(p),
        _ => return None,
    };
    Some(items)
}

/// Writes a scalar, an empty container, or a container's opening bracket
/// (returning its frame).
fn begin<'a>(
    parsed: &Parsed,
    value: &'a Value,
    level: usize,
    out: &mut impl Sink,
) -> Result<Option<Frame<'a>>, Unwritable> {
    let _ = parsed;
    match value {
        Value::Bool(b) => out.put(if *b { b"true" } else { b"false" }),
        Value::Int(i) => int((*i).into(), out),
        Value::UInt(u) => int((*u).into(), out),
        Value::F32(bits) => float(f32_value(*bits), out),
        Value::F64(bits) => float(f64::from_bits(*bits), out),
        Value::Str(s) => string(s.as_bytes(), out),
        Value::Bytes(_) => return Err(Unwritable),
        _ => {
            let Some(items) = open(value) else {
                return Err(Unwritable);
            };
            let dict = matches!(items, Items::Dict(_));
            if items.len() == 0 {
                out.put(if dict { b"{}" } else { b"[]" });
                return Ok(None);
            }
            out.put(if dict { b"{" } else { b"[" });
            return Ok(Some(Frame {
                items,
                next: 0,
                level,
            }));
        }
    }
    Ok(None)
}

/// Why a value was not written: a value `json.dumps` cannot write, or the
/// refused charge of the walk's stack.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// [`Unwritable`].
    #[error(transparent)]
    Unwritable(#[from] Unwritable),
    /// The stack's growth, refused by `meter` (or by the allocator, which
    /// the core reports as a budget's refusal).
    #[error("the JSON writer's stack: {0}")]
    Charge(#[from] ChargeError),
}

/// Writes `value`, read with `parsed`'s node lists (which name a dict's
/// keys), as it sits at `level` levels of indentation: its first line
/// continues the current one, its nested lines are indented below it. The
/// walk's stack, a frame per open container, is charged to `meter` before
/// it grows (review round 2's S3), and returned when the write ends.
pub fn value(
    parsed: &Parsed,
    value: &Value,
    level: usize,
    out: &mut impl Sink,
    meter: &Arc<Meter>,
) -> Result<(), WriteError> {
    let mut stack: MeteredVec<Frame<'_>> = MeteredVec::new(meter);
    if let Some(f) = begin(parsed, value, level, out)? {
        stack.push(f)?;
    }
    while let Some(top) = stack.as_mut_slice().last_mut() {
        let i = top.next;
        let inner = top.level + 1;
        if i >= top.items.len() {
            let close: &[u8] = if matches!(top.items, Items::Dict(_)) {
                b"}"
            } else {
                b"]"
            };
            let lvl = top.level;
            stack.truncate(stack.len().saturating_sub(1));
            newline(lvl, out);
            out.put(close);
            continue;
        }
        top.next += 1;
        if i > 0 {
            out.put(b",");
        }
        newline(inner, out);
        let child = match &top.items {
            Items::Prims(p) => {
                prim(p, i, out);
                None
            }
            Items::Seq(v) => v.get(i),
            Items::Dict(m) => match m.entries().get(i) {
                Some((k, v)) => {
                    string(parsed.key_name(m, *k), out);
                    out.put(b": ");
                    Some(v)
                }
                None => None,
            },
        };
        if let Some(v) = child
            && let Some(f) = begin(parsed, v, inner, out)?
        {
            stack.push(f)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Count, decode_escaped, float_repr, string};

    #[test]
    fn floats_are_written_as_python_reprs_them() {
        // Each pair is what CPython 3.11.18 in the oracle image gives for
        // `json.dumps(x)` (an `f32` as `struct` widens it).
        for (x, want) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e-05, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::from(f32::MAX), "3.4028234663852886e+38"),
            (f64::from(0.1f32), "0.10000000149011612"),
            (f64::from(1e-5f32), "9.999999747378752e-06"),
            (16_777_216.0, "16777216.0"),
            (5e-324, "5e-324"),
            (1.5, "1.5"),
            (-2.5e-7, "-2.5e-07"),
            (f64::MAX, "1.7976931348623157e+308"),
        ] {
            assert_eq!(float_repr(x), want, "{x:e}");
        }
    }

    #[test]
    fn ties_go_to_the_even_digits_and_powers_of_two_to_the_nearest() {
        // Doubles written exactly: two shortest digit strings equally near,
        // where CPython takes the even one (`{:e}` broke the tie upwards),
        // and powers of two, whose nearest shortest digits can fail to read
        // back (the dump fixture's `index.json` "str" cases).
        for (exact, want) in [
            ("123456789012345680.0", "1.2345678901234568e+17"),
            ("1551563132437320.25", "1551563132437320.2"),
            ("-1551563132437320.25", "-1551563132437320.2"),
            ("79686618578806.625", "79686618578806.62"),
            ("-22620844907483.3125", "-22620844907483.312"),
            ("2.98023223876953125e-8", "2.9802322387695312e-08"),
            ("-5.9604644775390625e-7", "-5.960464477539062e-07"),
            ("9007199254740992.0", "9007199254740992.0"),
        ] {
            let x: f64 = exact.parse().unwrap();
            assert_eq!(float_repr(x), want, "{exact}");
        }
    }

    fn json(bytes: &[u8]) -> String {
        let mut out = Vec::new();
        string(bytes, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn strings_escape_as_ensure_ascii_with_surrogateescape() {
        // CPython 3.11.18's `json.dumps` of each, decoded with
        // `surrogateescape`. `B` is a backslash, spelled so that no tool
        // reads the escapes as characters.
        const B: char = '\\';
        assert_eq!(json(b"a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(
            json(b"\n\r\t\x08\x0c\x01\x7f"),
            r#""\n\r\t\b\f\u0001\u007f""#
        );
        assert_eq!(
            json("\u{e4}\u{7cbe}".as_bytes()),
            format!("\"{B}u00e4{B}u7cbe\"")
        );
        assert_eq!(
            json("\u{1f600}".as_bytes()),
            format!("\"{B}ud83d{B}ude00\"")
        );
        assert_eq!(json(b"bad\xff"), r#""bad\udcff""#);
        // A truncated sequence is one escape per byte; decoding resumes.
        assert_eq!(json(b"\xe4\xb8a"), r#""\udce4\udcb8a""#);
    }

    #[test]
    fn decoding_gives_one_item_per_invalid_byte() {
        let mut n = 0;
        decode_escaped(b"\xf0\x80x\xc3", |_| n += 1);
        assert_eq!(n, 4);
        let mut c = Count::default();
        string(b"ab", &mut c);
        assert_eq!(c.0, 4);
    }
}
