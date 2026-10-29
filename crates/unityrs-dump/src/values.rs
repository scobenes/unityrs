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

use std::io::Write;
use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    L2Kind, ParseError, ReadCtx, ReadOptions, RefTypes, WriteCtx, WriteRules, read_typetree,
};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr};

use crate::encode::encode;
use crate::input::check_read_kind;
use crate::json::hex;

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

/// One case's outcome, as the protocol writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The L2 error kind, or `None`.
    pub kind: Option<&'static str>,
    /// For `check_read`.
    pub bytes_read: Option<i64>,
    /// The encoded value.
    pub value: Option<String>,
    /// `"skipped"`, `"equal"`, `"differs"` or `"error: ..."`.
    pub resave: String,
    /// The re-saved bytes.
    pub resave_hex: Option<String>,
    /// A message.
    pub note: Option<String>,
}

const _: () = crate::send_sync::<Outcome>();

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

/// The tree a case's rows build, as the script builds UnityPy's.
pub fn case_tree(case: &Case, limits: &Limits) -> Result<Arc<Tree>, String> {
    let rows = case
        .nodes
        .iter()
        .map(|(level, ty, name, meta)| NodeRow {
            level: *level,
            ty: RawStr::from(ty.as_str()),
            name: RawStr::from(name.as_str()),
            byte_size: -1,
            version: 1,
            type_flags: Some(0),
            meta_flag: Some(i64::from(*meta)),
            ..NodeRow::default()
        })
        .collect();
    let list = NodeList::trusted(rows).map_err(|e| e.to_string())?;
    Tree::compile(Arc::new(list), limits).map_err(|e| e.to_string())
}

/// Runs one case.
#[must_use]
pub fn run_case(case: &Case) -> Outcome {
    let mut out = Outcome {
        kind: None,
        bytes_read: None,
        value: None,
        resave: "skipped".to_owned(),
        resave_hex: None,
        note: None,
    };
    let limits = Limits::default();
    let tree = match case_tree(case, &limits) {
        Ok(t) => t,
        Err(e) => {
            out.kind = Some(L2Kind::Other.as_str());
            out.note = Some(e);
            return out;
        }
    };
    let endian = if case.big_endian {
        Endian::Big
    } else {
        Endian::Little
    };
    let meter = Meter::unlimited();
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let ctx = ReadCtx {
        endian,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    let data = Bytes::from(case.data.clone());
    let Ok(byte_size) = u32::try_from(data.len()) else {
        out.kind = Some(L2Kind::Other.as_str());
        out.note = Some("a case larger than 4 GiB".to_owned());
        return out;
    };
    let read = read_typetree(&data, 0, byte_size, &tree, &ctx, true);
    let (parsed, shared): (Option<Parsed>, Option<Arc<Parsed>>) = match read {
        Ok(p) => (Some(p), None),
        Err(e) => {
            out.note = Some(e.to_string());
            if let ParseError::CheckRead { parsed, .. } = e {
                let (kind, n) = check_read_kind(parsed.bytes_read);
                out.kind = Some(kind.as_str());
                out.bytes_read = n;
                if kind != L2Kind::CheckRead {
                    return out;
                }
                (None, Some(parsed))
            } else {
                out.kind = Some(e.l2_kind().as_str());
                return out;
            }
        }
    };
    let Some(p) = parsed.as_ref().or(shared.as_deref()) else {
        return out;
    };
    match encode(p) {
        Ok(v) => out.value = Some(v),
        Err(e) => {
            out.kind = Some(L2Kind::Encode.as_str());
            out.note = Some(e.to_string());
            return out;
        }
    }
    if out.kind.is_none() {
        let wctx = WriteCtx {
            endian,
            ref_types: RefTypes::List(&[]),
            rules: WriteRules::UnityPy,
            limits: &limits,
        };
        let written = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory)
            .map_err(|e| e.to_string())
            .and_then(|mut buf| {
                unityrs_core::walk::write::write(p, &tree, &wctx, &mut buf)
                    .map(|()| buf.as_slice().to_vec())
                    .map_err(|e| e.to_string())
            });
        match written {
            Ok(bytes) => {
                out.resave = String::from(if bytes == case.data {
                    "equal"
                } else {
                    "differs"
                });
                out.resave_hex = Some(hex(&bytes));
            }
            Err(e) => out.resave = format!("error: {e}"),
        }
    }
    out
}

/// The protocol's line for a case.
#[must_use]
pub fn outcome_line(name: &str, o: &Outcome) -> String {
    json!({
        "name": name,
        "kind": o.kind,
        "bytes_read": o.bytes_read,
        "value": o.value,
        "resave": o.resave,
        "resave_hex": o.resave_hex,
        "note": o.note,
    })
    .to_string()
}

/// Runs every case of a cases file, one line each to `out`.
pub fn run(text: &[u8], out: &mut dyn Write) -> Result<(), String> {
    for case in parse_cases(text)? {
        let o = run_case(&case);
        writeln!(out, "{}", outcome_line(&case.name, &o)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases_parse_as_the_script_writes_them() {
        let text = br#"[{"name":"c","endian":">","nodes":[[0,"Base","Base",0],[1,"int","a",16384]],"hex":"0001ff"}]"#;
        let cases = parse_cases(text).unwrap();
        assert_eq!(
            cases,
            [Case {
                name: "c".into(),
                big_endian: true,
                nodes: vec![
                    (0, "Base".into(), "Base".into(), 0),
                    (1, "int".into(), "a".into(), 16384)
                ],
                data: vec![0, 1, 0xff],
            }]
        );
        assert!(parse_cases(br#"[{"name":"c","nodes":[[0,"x"]],"hex":""}]"#).is_err());
        assert!(parse_cases(br#"[{"name":"c","nodes":[],"hex":"0"}]"#).is_err());
    }
}
