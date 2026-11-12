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
//! node list or key is not in its `Parsed`. Depth is not refused: the value
//! is walked with a stack of its own, not by recursion, so it encodes as
//! deep as any reader returns it under any limits, `--unbounded ValueDepth`
//! included (review round 11: a fixed bound of 4,096 levels made the lifted
//! dump write `encode` for values UnityPyBoost reads). That stack holds one
//! frame per open list, tuple or dict, charged to the sink's meter
//! ([`Sink::meter`]) before it grows, as the text is.

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::typetree::NodeList;
use unityrs_core::value::Map;
use unityrs_core::{Parsed, PrimArray, Value};

use crate::json::{push_hex, write_name, write_raw, write_value_str};
use crate::text::Sink;

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
///
/// # Errors
/// [`EncodeError`]: a map names a node list or a key its `Parsed` does not
/// hold, or the allocator refused the encoder's stack (a `String` holds no
/// refusal: [`Sink::stop`]).
pub fn encode(parsed: &Parsed) -> Result<String, EncodeError> {
    let mut out = String::new();
    encode_into(&mut out, parsed)?;
    Ok(out)
}

/// Appends `parsed`'s value as canonical JSON text to `out` (the dump's
/// records: a charged [`crate::text::Text`], whose refusal its caller
/// reads, the encoder's stack's included). On an error, or a refusal the
/// sink holds, `out` holds a partial encoding, which the caller drops.
///
/// # Errors
/// As [`encode`].
pub fn encode_into<S: Sink>(out: &mut S, parsed: &Parsed) -> Result<(), EncodeError> {
    let meter = out.meter().map_or_else(Meter::unlimited, Arc::clone);
    let mut open = MeteredVec::new(&meter);
    match walk(out, parsed, &mut open) {
        Ok(()) => Ok(()),
        Err(Stop::Encode(e)) => Err(e),
        Err(Stop::Memory(e)) => out.stop(e).map_or(Ok(()), |e| {
            Err(EncodeError(format!(
                "the encoder's stack of open values was refused: {e}"
            )))
        }),
    }
}

/// Why a walk stopped.
enum Stop {
    Encode(EncodeError),
    /// The stack's growth was refused.
    Memory(ChargeError),
}

impl From<EncodeError> for Stop {
    fn from(e: EncodeError) -> Self {
        Self::Encode(e)
    }
}

impl From<ChargeError> for Stop {
    fn from(e: ChargeError) -> Self {
        Self::Memory(e)
    }
}

/// A list, tuple or dict whose opening is written and whose items are not
/// all written yet.
enum Frame<'a> {
    /// A JSON array's items (a list's, or a tuple's `v`), and what closes
    /// the value once they are written.
    Items {
        items: std::slice::Iter<'a, Value>,
        first: bool,
        close: &'static str,
    },
    /// A dict's entries, the list naming their keys, and whether the entry
    /// last begun still needs its `]` (its value is written by the time
    /// this frame is on top again).
    Entries {
        entries: std::slice::Iter<'a, (unityrs_core::typetree::NodeId, Value)>,
        list: &'a NodeList,
        first: bool,
        in_entry: bool,
    },
}

/// Writes `p`'s value: each scalar where it stands, each list, tuple and
/// dict as its opening, a frame on `open` for its items, and its closing
/// once the frame runs out.
fn walk<'a, S: Sink>(
    out: &mut S,
    p: &'a Parsed,
    open: &mut MeteredVec<Frame<'a>>,
) -> Result<(), Stop> {
    begin(out, p, p.value(), open)?;
    while let Some(top) = open.as_mut_slice().last_mut() {
        let next = match top {
            Frame::Items {
                items,
                first,
                close,
            } => {
                let next = items.next();
                if next.is_none() {
                    out.push_str(close);
                } else if !std::mem::take(first) {
                    out.push(',');
                }
                next
            }
            Frame::Entries {
                entries,
                list,
                first,
                in_entry,
            } => {
                if std::mem::take(in_entry) {
                    out.push(']');
                }
                if let Some((key, item)) = entries.next() {
                    let row = list.row(*key).ok_or_else(|| {
                        EncodeError(format!(
                            "a map key names node {} of a list of {}",
                            key.0,
                            list.len()
                        ))
                    })?;
                    if !std::mem::take(first) {
                        out.push(',');
                    }
                    out.push('[');
                    write_name(out, row.name.as_bytes());
                    out.push(',');
                    *in_entry = true;
                    Some(item)
                } else {
                    out.push_str("]}");
                    None
                }
            }
        };
        match next {
            Some(item) => begin(out, p, item, open)?,
            None => open.truncate(open.len().saturating_sub(1)),
        }
    }
    Ok(())
}

/// Writes `v` if it is a scalar or a packed run, else its opening, and
/// pushes the frame that writes the rest.
fn begin<'a, S: Sink>(
    out: &mut S,
    p: &'a Parsed,
    v: &'a Value,
    open: &mut MeteredVec<Frame<'a>>,
) -> Result<(), Stop> {
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
        Value::List(items) => {
            if !packed(out, items) {
                out.push('[');
                open.push(Frame::Items {
                    items: items.iter(),
                    first: true,
                    close: "]",
                })?;
            }
        }
        Value::Tuple(items) => {
            out.push_str("{\"t\":\"tuple\",\"v\":[");
            open.push(Frame::Items {
                items: items.iter(),
                first: true,
                close: "]}",
            })?;
        }
        Value::Dict(m) => {
            let list = node_list(p, m)?;
            out.push_str("{\"t\":\"dict\",\"v\":[");
            open.push(Frame::Entries {
                entries: m.entries().iter(),
                list,
                first: true,
                in_entry: false,
            })?;
        }
    }
    Ok(())
}

/// `_dict`: the list that names `m`'s keys, by the metadata-string rule.
fn node_list<'a>(p: &'a Parsed, m: &Map) -> Result<&'a NodeList, EncodeError> {
    usize::try_from(m.list())
        .ok()
        .and_then(|i| p.lists().get(i))
        .map(|l| &**l)
        .ok_or_else(|| {
            EncodeError(format!(
                "a map names node list {} of {}",
                m.list(),
                p.lists().len()
            ))
        })
}

/// `_seq` on a list read item by item: packed when it is not empty and
/// every item is a float of one width (the element node is `float` or
/// `double`); `false`, with nothing written, for every other list.
fn packed<S: Sink>(out: &mut S, items: &[Value]) -> bool {
    if items.is_empty() {
        return false;
    }
    if items.iter().all(|v| matches!(v, Value::F32(_))) {
        out.push_str("{\"t\":\"f32s\",\"v\":\"");
        for v in items {
            if let Value::F32(bits) = v {
                let _ = write!(out, "{bits:08x}");
            }
        }
        out.push_str("\"}");
        return true;
    }
    if items.iter().all(|v| matches!(v, Value::F64(_))) {
        out.push_str("{\"t\":\"f64s\",\"v\":\"");
        for v in items {
            if let Value::F64(bits) = v {
                let _ = write!(out, "{bits:016x}");
            }
        }
        out.push_str("\"}");
        return true;
    }
    false
}

/// `_seq` on a primitive run: an empty one is `[]`; `float`, `double` and
/// `UInt8`/`char` runs pack; every other run is a JSON array.
fn prims<S: Sink>(out: &mut S, a: &PrimArray) {
    fn ints<S: Sink, T: std::fmt::Display>(out: &mut S, v: &[T]) {
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

#[cfg(test)]
mod tests {
    use std::fmt;

    use unityrs_core::LimitKind;
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::limits::heap_cost;
    use unityrs_core::typetree::{NodeId, NodeRow};
    use unityrs_core::{Built, ValueBuilder};

    use super::*;

    /// How deep each test value nests: far past the 4,096 levels the
    /// encoder once refused (review round 11), and past what a recursion
    /// fits on [`SMALL`].
    const LEVELS: usize = 100_000;
    /// The stack every encoding here runs on.
    const SMALL: usize = 128 << 10;
    /// The stack a value is built and dropped on: a value is dropped one
    /// level per frame (`Limits::stack_needed`).
    const BIG: usize = 512 << 20;

    /// A value [`LEVELS`] deep, a list, a tuple and a dict in turn around
    /// the int 1, and its encoding by `docs/CANONICAL_DUMP.md` section 3.
    fn deep() -> (Parsed, String) {
        let rows = vec![NodeRow::new(0, "Base", "Base"), NodeRow::new(1, "int", "k")];
        let b = ValueBuilder::new(
            &Meter::unlimited(),
            Arc::new(NodeList::trusted(rows).unwrap()),
        )
        .unwrap();
        let mut v: Built = b.scalar(Value::Int(1)).unwrap();
        let (mut head, mut tail) = (Vec::new(), String::new());
        for level in 0..LEVELS {
            let (opening, closing) = match level % 3 {
                0 => {
                    let mut items = b.vec();
                    items.push_built(v).unwrap();
                    v = b.list(items);
                    ("[", "]")
                }
                1 => {
                    let mut items = b.vec();
                    items.push_built(v).unwrap();
                    v = b.tuple(items).unwrap();
                    ("{\"t\":\"tuple\",\"v\":[", "]}")
                }
                _ => {
                    let mut entries = b.vec();
                    entries.push_entry(NodeId(1), v).unwrap();
                    v = b.map(0, entries);
                    ("{\"t\":\"dict\",\"v\":[[\"k\",", "]]}")
                }
            };
            head.push(opening);
            tail.push_str(closing);
        }
        head.reverse();
        (b.finish(v, 0), head.concat() + "1" + &tail)
    }

    /// Runs `f` on a value from [`deep`], on a thread of [`SMALL`] stack,
    /// the value built and dropped on one of [`BIG`].
    fn on_deep<R: Send>(f: impl FnOnce(&Parsed, &str) -> R + Send) -> R {
        let big = std::thread::Builder::new().stack_size(BIG);
        std::thread::scope(|s| {
            big.spawn_scoped(s, || {
                let (parsed, want) = deep();
                let small = std::thread::Builder::new().stack_size(SMALL);
                std::thread::scope(|s| {
                    small
                        .spawn_scoped(s, || f(&parsed, &want))
                        .unwrap()
                        .join()
                        .unwrap()
                })
            })
            .unwrap()
            .join()
            .unwrap()
        })
    }

    /// Review round 11: `--unbounded ValueDepth` lets a reader return a
    /// value deeper than the encoder's old bound, which then wrote `encode`.
    /// A value of any depth encodes, on a stack a recursion of that depth
    /// would overflow.
    #[test]
    fn a_value_of_any_depth_encodes_without_recursion() {
        on_deep(|parsed, want| assert!(encode(parsed).unwrap() == want, "not the canonical text"));
    }

    /// A sink with a meter that writes into a `String`: only the encoder's
    /// stack is charged to the meter.
    struct Metered {
        text: String,
        meter: Arc<Meter>,
        stopped: Option<ChargeError>,
    }

    impl fmt::Write for Metered {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.text.push_str(s);
            Ok(())
        }
    }

    impl Sink for Metered {
        fn push_str(&mut self, s: &str) {
            self.text.push_str(s);
        }
        fn push(&mut self, c: char) {
            self.text.push(c);
        }
        fn meter(&self) -> Option<&Arc<Meter>> {
            Some(&self.meter)
        }
        fn stop(&mut self, e: ChargeError) -> Option<ChargeError> {
            self.stopped = Some(e);
            None
        }
    }

    /// The stack holds a frame a level, charged to the sink's meter before
    /// it grows and returned after: a meter that holds half the frames
    /// stops the sink with its refusal, and one that holds them all twice
    /// over encodes the value.
    #[test]
    fn the_stack_is_charged_to_the_sinks_meter() {
        let frames = |n: usize| heap_cost(u64::try_from(n * size_of::<Frame>()).unwrap());
        on_deep(|parsed, want| {
            let meter = Meter::new(frames(LEVELS / 2), Arc::new(Unbounded), Reservation::Input);
            let mut sink = Metered {
                text: String::new(),
                meter: meter.clone(),
                stopped: None,
            };
            assert_eq!(encode_into(&mut sink, parsed), Ok(()));
            assert!(matches!(
                sink.stopped,
                Some(ChargeError::Limit(ref l)) if l.kind() == LimitKind::InputMemory
            ));
            assert_eq!(meter.used(), 0);

            let meter = Meter::new(frames(LEVELS) * 4, Arc::new(Unbounded), Reservation::Input);
            let mut sink = Metered {
                text: String::new(),
                meter: meter.clone(),
                stopped: None,
            };
            assert_eq!(encode_into(&mut sink, parsed), Ok(()));
            assert!(sink.stopped.is_none() && sink.text == want);
            assert_eq!(meter.used(), 0);
        });
    }
}
