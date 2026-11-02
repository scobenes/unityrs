//! `unityrs values CASES.json`: the value-case protocol of the P0 spike
//! (`rabex-dump values`), which `tools/parity/value_cases.py` drives (plan
//! section 5, gate G2).
//!
//! In: a JSON list of cases, each `{"name", "endian", "nodes", "hex"}`,
//! `nodes` being `[level, type, name, meta_flag]` rows. Out: one JSON line
//! per case, `{"name", "kind", "bytes_read", "value", "resave",
//! "resave_hex", "note"}`, which the script compares with UnityPy's
//! `read_typetree` and `write_typetree` on the same case
//! (`value_cases.py` `oracle`):
//! - the tree is built from the rows as the script builds UnityPy's
//!   (`TypeTreeNode(m_Level, m_Type, m_Name, m_ByteSize=-1, m_Version=1,
//!   m_MetaFlag, m_TypeFlags=0)` through `from_list`);
//! - the read is `read_typetree(..., byte_size=len(data), check_read=True)`
//!   with a file whose `ref_types` is empty; its error's kind is the dump's
//!   (a negative `check_read` is `other`, [`crate::input::check_read_kind`]);
//!   a `check_read` keeps the value;
//! - the value is encoded by [`crate::encode`]; an encoder refusal is kind
//!   `encode`;
//! - a read with no error is re-saved with `WriteRules::UnityPy`: `resave`
//!   is `"equal"`, `"differs"` or `"error: ..."`, and `resave_hex` the bytes.
//!
//! **Memory**, as the dump's ([`crate::input`], "Memory"). A case's bytes
//! can read as far more than they are, and its value's text can be larger
//! still: 16 bytes whose vector count, read past an unaligned
//! `TypelessData`, is `0x03000000` read as that many empty structs, on an
//! unlimited meter, and their text, grown in a plain `String`, aborted the
//! process when the allocator refused it (review round 15; the tests'
//! `a_read_past_the_cases_memory_bound_is_its_outcome` reads that case,
//! and `unityrs-cli`'s `tests/values.rs` runs it through the binary). So
//! each case runs on a meter of its own ([`Meter::for_input`], under the
//! limits and the budget of [`FileContext::unitypy`], the dump's), which its
//! bytes, its tree's rows and names, its read, its value's text (a [`Text`])
//! and its re-saved bytes (a [`DecodeBuf`]) are charged to before they are
//! allocated, each allocated fallibly. Its line is written to the output
//! from those, member by member, so the line itself is never held, and
//! flushed; what the case held is returned before the next case runs
//! ([`run_cases`]). A refusal is the case's outcome, never an abort of the
//! run, and every other case is still answered:
//! - by a bound of the limits (`max_input_memory` among them): kind
//!   `other`, its note holding the limit's text, `limit <Kind>: <value> >
//!   <max>`, as a read refused at a bound always was;
//! - of memory, by the budget or the allocator: kind [`CRASH`], with no
//!   value and no resave, as the dump makes such an input its `crash`
//!   record (`docs/CANONICAL_DUMP.md` section 4.2): it says more about the
//!   machine than about the case.
//!
//! What a run holds beside its cases' meters is the cases file and its
//! parse, in proportion to the file's length: the caller's own file, as
//! the dump's index is.

use std::io::{self, Write};
use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::budget::{BudgetExceeded, DecodeBuf, MemoryBudget, Reservation};
use unityrs_core::limits::{Charge, ChargeError, Limits, Meter, MeteredVec};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree, TreeError};
use unityrs_core::walk::{
    L2Kind, ParseError, ReadCtx, ReadOptions, RefTypes, WriteCtx, WriteRules, read_typetree,
};
use unityrs_core::{Error, FileContext, LimitKind, Parsed, RawStr};

use crate::encode::encode_into;
use crate::input::{check_read_kind, out_of_memory, write_out_of_memory};
use crate::text::Text;

/// The kind of a case refused memory by the budget or the allocator (module
/// docs): no L2 kind, as the dump's `crash` record is no object's outcome.
pub const CRASH: &str = "crash";

/// One case of the protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    /// Its name.
    pub name: String,
    /// `"<"` or `">"`.
    pub big_endian: bool,
    /// `[level, type, name, meta_flag]` rows.
    pub nodes: Vec<(i32, String, String, i32)>,
    /// The object's bytes.
    pub data: Vec<u8>,
}

const _: () = crate::send_sync::<Case>();

/// One case's outcome, as the protocol writes it, holding what it keeps
/// on the case's meter: its value's text and its re-saved bytes, each with
/// its charge.
#[derive(Debug)]
pub struct Outcome {
    /// The L2 error kind, [`CRASH`], or `None`.
    pub kind: Option<&'static str>,
    /// For `check_read`.
    pub bytes_read: Option<i64>,
    /// The encoded value, and the charge for its text.
    pub value: Option<(String, Charge)>,
    /// `"skipped"`, `"equal"`, `"differs"` or `"error: ..."`.
    pub resave: String,
    /// The re-saved bytes (the line's `resave_hex`), in the charged buffer
    /// the writer wrote them to.
    pub resaved: Option<DecodeBuf>,
    /// A message.
    pub note: Option<String>,
}

const _: () = crate::send_sync::<Outcome>();

impl Outcome {
    fn empty() -> Self {
        Self {
            kind: None,
            bytes_read: None,
            value: None,
            resave: "skipped".to_owned(),
            resaved: None,
            note: None,
        }
    }

    /// The case's only outcome, refused `e`: kind `other` with the limit's
    /// text, or [`CRASH`] (module docs).
    fn refused(e: Stop) -> Self {
        let mut out = Self::empty();
        match e {
            Stop::Limit(note) => {
                out.kind = Some(L2Kind::Other.as_str());
                out.note = Some(note);
            }
            Stop::Memory(b) => {
                out.kind = Some(CRASH);
                out.note = Some(format!("unityrs ran out of memory on this case ({b})"));
            }
        }
        out
    }

    /// The encoded value's text.
    #[must_use]
    pub fn value_text(&self) -> Option<&str> {
        self.value.as_ref().map(|(s, _)| s.as_str())
    }

    /// The re-saved bytes.
    #[must_use]
    pub fn resave_bytes(&self) -> Option<&[u8]> {
        self.resaved.as_ref().map(DecodeBuf::as_slice)
    }
}

/// What ends a case before its outcome: a refusal (module docs).
enum Stop {
    /// By a bound of the limits: the note.
    Limit(String),
    /// Of memory, by the budget or the allocator.
    Memory(BudgetExceeded),
}

impl From<ChargeError> for Stop {
    fn from(e: ChargeError) -> Self {
        match e {
            ChargeError::Limit(l) => Self::Limit(l.to_string()),
            ChargeError::Budget(b) => Self::Memory(b),
        }
    }
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd-length hex {s:?}"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
                .ok_or_else(|| format!("bad hex {s:?}"))
        })
        .collect()
}

/// Parses a cases file.
pub fn parse_cases(text: &[u8]) -> Result<Vec<Case>, String> {
    let all: Json = serde_json::from_slice(text).map_err(|e| e.to_string())?;
    let list = all.as_array().ok_or("cases is not a list")?;
    list.iter()
        .map(|c| {
            let name = c
                .get("name")
                .and_then(Json::as_str)
                .ok_or("a case without a name")?
                .to_owned();
            let nodes = c
                .get("nodes")
                .and_then(Json::as_array)
                .ok_or_else(|| format!("{name}: no nodes"))?
                .iter()
                .map(|r| {
                    let f = |i: usize| r.get(i).ok_or_else(|| format!("{name}: a short node row"));
                    let int = |i: usize| -> Result<i32, String> {
                        f(i)?
                            .as_i64()
                            .and_then(|v| i32::try_from(v).ok())
                            .ok_or_else(|| format!("{name}: a node field is not an i32"))
                    };
                    let text = |i: usize| -> Result<String, String> {
                        Ok(f(i)?
                            .as_str()
                            .ok_or_else(|| format!("{name}: a node name is not a string"))?
                            .to_owned())
                    };
                    Ok((int(0)?, text(1)?, text(2)?, int(3)?))
                })
                .collect::<Result<_, String>>()?;
            let data = unhex(c.get("hex").and_then(Json::as_str).unwrap_or_default())?;
            Ok(Case {
                big_endian: c.get("endian").and_then(Json::as_str) == Some(">"),
                name,
                nodes,
                data,
            })
        })
        .collect()
}

/// The tree a case's rows build, as the script builds UnityPy's: the rows,
/// their names and the tree charged to `meter`.
///
/// # Errors
/// The rows do not make a tree, or a charge was refused.
pub fn case_tree(case: &Case, limits: &Limits, meter: &Arc<Meter>) -> Result<Arc<Tree>, TreeError> {
    let mut rows = MeteredVec::new(meter);
    rows.reserve_exact(case.nodes.len())?;
    for (level, ty, name, meta) in &case.nodes {
        rows.push(NodeRow {
            level: *level,
            ty: RawStr::charged(ty.as_bytes(), meter)?,
            name: RawStr::charged(name.as_bytes(), meter)?,
            byte_size: -1,
            version: 1,
            type_flags: Some(0),
            meta_flag: Some(i64::from(*meta)),
            ..NodeRow::default()
        })?;
    }
    Tree::compile(Arc::new(NodeList::from_rows(rows)?), limits)
}

/// Runs one case on a meter of its own under `limits`, booked with
/// `budget` (module docs, "Memory").
#[must_use]
pub fn run_case_on(case: &Case, limits: &Limits, budget: &Arc<dyn MemoryBudget>) -> Outcome {
    let meter = Meter::for_input(limits, Arc::clone(budget), Reservation::Input);
    outcome_on(case, limits, &meter).unwrap_or_else(Outcome::refused)
}

/// `Err(Stop::Memory)` when `e` is a refusal of memory by the budget or the
/// allocator, as the dump reads one ([`out_of_memory`]).
fn memory(e: Result<(), Error>) -> Result<(), Stop> {
    match e {
        Err(Error::Budget(b)) => Err(Stop::Memory(b)),
        _ => Ok(()),
    }
}

fn outcome_on(case: &Case, limits: &Limits, meter: &Arc<Meter>) -> Result<Outcome, Stop> {
    let mut out = Outcome::empty();
    let tree = match case_tree(case, limits, meter) {
        Ok(t) => t,
        Err(e) => {
            let e = ParseError::Tree(e);
            memory(out_of_memory(&e))?;
            out.kind = Some(L2Kind::Other.as_str());
            out.note = Some(e.to_string());
            return Ok(out);
        }
    };
    let endian = if case.big_endian {
        Endian::Big
    } else {
        Endian::Little
    };
    let options = ReadOptions {
        limits: *limits,
        ..ReadOptions::default()
    };
    let ctx = ReadCtx {
        endian,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter,
    };
    let Ok(byte_size) = u32::try_from(case.data.len()) else {
        out.kind = Some(L2Kind::Other.as_str());
        out.note = Some("a case larger than 4 GiB".to_owned());
        return Ok(out);
    };
    // The case's bytes, charged as the dump charges an input's and copied
    // fallibly; a charge they carry returns when the last slice of them drops.
    let data = RawStr::charged(&case.data, meter)?.into_bytes();
    let read = read_typetree(&data, 0, byte_size, &tree, &ctx, true);
    let (parsed, shared): (Option<Parsed>, Option<Arc<Parsed>>) = match read {
        Ok(p) => (Some(p), None),
        Err(e) => {
            memory(out_of_memory(&e))?;
            out.note = Some(e.to_string());
            if let ParseError::CheckRead { parsed, .. } = e {
                let (kind, n) = check_read_kind(parsed.bytes_read);
                out.kind = Some(kind.as_str());
                out.bytes_read = n;
                if kind != L2Kind::CheckRead {
                    return Ok(out);
                }
                (None, Some(parsed))
            } else {
                out.kind = Some(e.l2_kind().as_str());
                return Ok(out);
            }
        }
    };
    let Some(p) = parsed.as_ref().or(shared.as_deref()) else {
        return Ok(out);
    };
    let mut text = Text::on(meter);
    let encoded = encode_into(&mut text, p);
    // A refused growth ends the case, whatever the encoder said.
    let (text, charge) = text.finish()?;
    match encoded {
        Ok(()) => out.value = Some((text, charge)),
        Err(e) => {
            out.kind = Some(L2Kind::Encode.as_str());
            out.note = Some(e.to_string());
            return Ok(out);
        }
    }
    if out.kind.is_none() {
        let wctx = WriteCtx {
            endian,
            ref_types: RefTypes::List(&[]),
            rules: WriteRules::UnityPy,
            limits,
        };
        let mut buf = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
        match unityrs_core::walk::write::write(p, &tree, &wctx, &mut buf) {
            Ok(()) => {
                out.resave = String::from(if buf.as_slice() == case.data.as_slice() {
                    "equal"
                } else {
                    "differs"
                });
                out.resaved = Some(buf);
            }
            Err(e) => {
                memory(write_out_of_memory(&e))?;
                out.resave = format!("error: {e}");
            }
        }
    }
    Ok(out)
}

/// `bytes` as lowercase hex, written to `out` through a buffer on the stack.
fn write_hex<W: Write + ?Sized>(out: &mut W, bytes: &[u8]) -> io::Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digit = |n: u8| HEX.get(usize::from(n & 15)).copied().unwrap_or(b'0');
    let mut buf = [0u8; 1024];
    for chunk in bytes.chunks(buf.len() / 2) {
        for ([hi, lo], b) in buf.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
            *hi = digit(b >> 4);
            *lo = digit(*b);
        }
        out.write_all(buf.get(..2 * chunk.len()).unwrap_or_default())?;
    }
    Ok(())
}

/// Writes the protocol's line for a case to `out`, member by member, from
/// what the outcome holds: nothing the size of the line is allocated.
///
/// # Errors
/// What writing to `out` returns.
pub fn write_outcome<W: Write + ?Sized>(out: &mut W, name: &str, o: &Outcome) -> io::Result<()> {
    out.write_all(b"{\"name\":")?;
    serde_json::to_writer(&mut *out, name)?;
    out.write_all(b",\"kind\":")?;
    serde_json::to_writer(&mut *out, &o.kind)?;
    out.write_all(b",\"bytes_read\":")?;
    serde_json::to_writer(&mut *out, &o.bytes_read)?;
    out.write_all(b",\"value\":")?;
    serde_json::to_writer(&mut *out, &o.value_text())?;
    out.write_all(b",\"resave\":")?;
    serde_json::to_writer(&mut *out, &o.resave)?;
    out.write_all(b",\"resave_hex\":")?;
    match o.resave_bytes() {
        Some(bytes) => {
            out.write_all(b"\"")?;
            write_hex(out, bytes)?;
            out.write_all(b"\"")?;
        }
        None => out.write_all(b"null")?,
    }
    out.write_all(b",\"note\":")?;
    serde_json::to_writer(&mut *out, &o.note)?;
    out.write_all(b"}\n")
}

/// Runs `cases` one by one, each on a meter of its own under `limits`,
/// booked with `budget`, on the calling thread: each case's line is written
/// and flushed to `out` as the case ends, so a process killed from outside
/// on a later case has answered this one, and what the case held is returned
/// before the next one runs.
///
/// # Errors
/// What writing to `out` returns.
pub fn run_cases<W: Write + ?Sized>(
    cases: &[Case],
    limits: &Limits,
    budget: &Arc<dyn MemoryBudget>,
    out: &mut W,
) -> Result<(), String> {
    for c in cases {
        let o = run_case_on(c, limits, budget);
        write_outcome(out, &c.name, &o)
            .and_then(|()| out.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Runs every case of a cases file under the dump's limits and budget
/// ([`FileContext::unitypy`]), one line each to `out` ([`run_cases`]). The
/// cases run on a thread of their own, of the stack a dump worker has
/// ([`crate::run::worker_stack`]), so a case as deep as the limits let a
/// value be fits whatever stack the caller has.
///
/// # Errors
/// The cases file is not a list of cases, the thread does not start, a case
/// panics, or writing to `out` fails.
pub fn run<W: Write + Send + ?Sized>(text: &[u8], out: &mut W) -> Result<(), String> {
    let cases = parse_cases(text)?;
    let ctx = FileContext::unitypy();
    let stack = crate::run::worker_stack(&ctx.limits);
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(s, || run_cases(&cases, &ctx.limits, &ctx.budget, out))
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "a case panicked".to_owned())?
    })
}

#[cfg(test)]
mod tests;
