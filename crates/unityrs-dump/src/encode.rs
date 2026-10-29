//! The value encoder (`docs/CANONICAL_DUMP.md` section 3), from a
//! [`Parsed`] alone.
//!
//! `tools/parity/canonical.py`'s `ValueEncoder` reads the float width, the
//! packing and the dict keys' nodes from the typetree, because a Python
//! value does not carry them. A [`Value`] does (plan section 3.9): `F32` and
//! `F64` are the float node's width, a primitive run is a
//! [`PrimArray`] of its element type, and a map's keys are node ids of the
//! list [`Map::list`] names. So this encoder needs no tree, and each rule
//! below is the `ValueEncoder` rule it ports, expressed on the value:
//!
//! - `encode` (`canonical.py` `ValueEncoder.encode`): `int` and `bool` bare,
//!   `str` by `enc_str`, `bytes` by `enc_raw("bytes")`;
//! - `_float`: `f32` from a `float` node, `f64` from a `double` node, each the
//!   value's bit pattern (`f32_hex`, `f64_hex`);
//! - `_seq`: a non-empty list whose element node is `float` or `double` packs
//!   as `f32s`/`f64s`, one whose element node is `UInt8` or `char` as `u8s`
//!   (`enc_raw("u8s")`), every other list is a JSON array, and an empty list
//!   is always `[]`. Here: `Prims(F32)`/`Prims(F64)`, and a `List` whose
//!   elements are all `F32` or all `F64` (only a `float` or `double` element
//!   node gives those), pack as floats; `Prims(U8)` (the reader's form of a
//!   `UInt8` or `char` vector, plan section 3.9) packs as `u8s`;
//! - `_tuple`: `{"t":"tuple","v":[...]}`, never packed;
//! - `_dict`: `{"t":"dict","v":[[K,V],...]}` in the dict's order, each key by
//!   `enc_name` (the metadata-string rule).
//!
//! What the encoder can refuse ([`EncodeError`], the dump's L2 `encode`
//! kind, section 4.3) is a value the reader could not have made: a map whose
//! node list or key is not in its `Parsed`, or nesting deeper than
//! [`MAX_DEPTH`] (the reader's own bound, `Limits::max_value_depth`, is far
//! below it).

use std::fmt::Write as _;

use unityrs_core::value::Map;
use unityrs_core::{Parsed, PrimArray, Value};

use crate::json::{push_hex, write_name, write_raw, write_value_str};

/// How deep a value may nest before the encoder refuses it. The reader stops
/// at `Limits::max_value_depth` (512 by default); this bounds the encoder's
/// recursion for a value a caller built by hand.
pub const MAX_DEPTH: usize = 4096;

/// A value the canonical form cannot represent (a bug in the producer, never
/// an outcome of the input: section 4.3's `encode`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodeError(pub String);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}

const _: () = crate::send_sync::<EncodeError>();

/// Encodes `parsed`'s value as canonical JSON text.
pub fn encode(parsed: &Parsed) -> Result<String, EncodeError> {
    let mut out = String::new();
    encode_into(&mut out, parsed)?;
    Ok(out)
}

/// Appends `parsed`'s value as canonical JSON text to `out`. On an error
/// `out` holds a partial encoding, which the caller drops.
pub fn encode_into(out: &mut String, parsed: &Parsed) -> Result<(), EncodeError> {
    value(out, parsed, parsed.value(), 0)
}

fn value(out: &mut String, p: &Parsed, v: &Value, depth: usize) -> Result<(), EncodeError> {
    if depth > MAX_DEPTH {
        return Err(EncodeError(format!(
            "value nested deeper than {MAX_DEPTH} levels"
        )));
    }
    match v {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::UInt(u) => {
            let _ = write!(out, "{u}");
        }
        Value::F32(bits) => {
            let _ = write!(out, "{{\"t\":\"f32\",\"v\":\"{bits:08x}\"}}");
        }
        Value::F64(bits) => {
            let _ = write!(out, "{{\"t\":\"f64\",\"v\":\"{bits:016x}\"}}");
        }
        Value::Str(s) => write_value_str(out, s.as_bytes()),
        Value::Bytes(b) => write_raw(out, "bytes", b),
        Value::Prims(a) => prims(out, a),
        Value::List(items) => list(out, p, items, depth)?,
        Value::Tuple(items) => {
            out.push_str("{\"t\":\"tuple\",\"v\":");
            seq(out, p, items, depth)?;
            out.push('}');
        }
        Value::Dict(m) => dict(out, p, m, depth)?,
    }
    Ok(())
}

fn seq(out: &mut String, p: &Parsed, items: &[Value], depth: usize) -> Result<(), EncodeError> {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        value(out, p, item, depth + 1)?;
    }
    out.push(']');
    Ok(())
}

/// `_seq` on a list read item by item: packed when every item is a float of
/// one width (the element node is `float` or `double`), else an array.
fn list(out: &mut String, p: &Parsed, items: &[Value], depth: usize) -> Result<(), EncodeError> {
    if !items.is_empty() {
        if items.iter().all(|v| matches!(v, Value::F32(_))) {
            out.push_str("{\"t\":\"f32s\",\"v\":\"");
            for v in items {
                if let Value::F32(bits) = v {
                    let _ = write!(out, "{bits:08x}");
                }
            }
            out.push_str("\"}");
            return Ok(());
        }
        if items.iter().all(|v| matches!(v, Value::F64(_))) {
            out.push_str("{\"t\":\"f64s\",\"v\":\"");
            for v in items {
                if let Value::F64(bits) = v {
                    let _ = write!(out, "{bits:016x}");
                }
            }
            out.push_str("\"}");
            return Ok(());
        }
    }
    seq(out, p, items, depth)
}

/// `_seq` on a primitive run: an empty one is `[]`; `float`, `double` and
/// `UInt8`/`char` runs pack; every other run is a JSON array.
fn prims(out: &mut String, a: &PrimArray) {
    fn ints<T: std::fmt::Display>(out: &mut String, v: &[T]) {
        out.push('[');
        for (i, x) in v.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{x}");
        }
        out.push(']');
    }
    if a.is_empty() {
        out.push_str("[]");
        return;
    }
    match a {
        PrimArray::U8(b) => write_raw(out, "u8s", b),
        PrimArray::F32(v) => {
            out.push_str("{\"t\":\"f32s\",\"v\":\"");
            for bits in v {
                push_hex(out, &bits.to_be_bytes());
            }
            out.push_str("\"}");
        }
        PrimArray::F64(v) => {
            out.push_str("{\"t\":\"f64s\",\"v\":\"");
            for bits in v {
                push_hex(out, &bits.to_be_bytes());
            }
            out.push_str("\"}");
        }
        PrimArray::Bool(v) => {
            out.push('[');
            for (i, b) in v.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(if *b { "true" } else { "false" });
            }
            out.push(']');
        }
        PrimArray::I8(v) => ints(out, v),
        PrimArray::U16(v) => ints(out, v),
        PrimArray::I16(v) => ints(out, v),
        PrimArray::U32(v) => ints(out, v),
        PrimArray::I32(v) => ints(out, v),
        PrimArray::U64(v) => ints(out, v),
        PrimArray::I64(v) => ints(out, v),
    }
}

/// `_dict`: each key's name from the list the map names, by the
/// metadata-string rule.
fn dict(out: &mut String, p: &Parsed, m: &Map, depth: usize) -> Result<(), EncodeError> {
    let list = usize::try_from(m.list())
        .ok()
        .and_then(|i| p.lists().get(i))
        .ok_or_else(|| {
            EncodeError(format!(
                "a map names node list {} of {}",
                m.list(),
                p.lists().len()
            ))
        })?;
    out.push_str("{\"t\":\"dict\",\"v\":[");
    for (i, (key, item)) in m.entries().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let row = list.row(*key).ok_or_else(|| {
            EncodeError(format!(
                "a map key names node {} of a list of {}",
                key.0,
                list.len()
            ))
        })?;
        out.push('[');
        write_name(out, row.name.as_bytes());
        out.push(',');
        value(out, p, item, depth + 1)?;
        out.push(']');
    }
    out.push_str("]}");
    Ok(())
}
