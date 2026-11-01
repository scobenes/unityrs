//! Review round 6's sweep of the writer: work a write did once for each
//! write, in proportion to a name's or a string's length, that its open's
//! work did not see. A string's bytes are copied into the output and, when a
//! repeated name hands the string to a `pair` or an array sibling, passed
//! over to find its characters; both now count each byte as the open's work
//! first. A child's name is cleaned for `getattr` from at most its first
//! `CLEAN_MAX + 8` bytes, in [`STEPS`] that do not grow with the name.
//! Review round 7's sweep: a write of a class whose children repeat a name
//! no longer passes over them before it writes the first.

use std::sync::Arc;

use bytes::Bytes;

use super::super::dict::STEPS;
use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::limits::{Limits, Meter};
use crate::stream::Endian;
use crate::typetree::{NodeList, NodeRow, Tree};
use crate::value::Parsed;
use crate::walk::{ReadCtx, ReadOptions, RefTypes, WriteCtx, WriteError, WriteRules, python};

const LONG: usize = 1 << 16;

fn tree(rows: Vec<NodeRow>) -> Arc<Tree> {
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

fn read(tree: &Arc<Tree>, data: Vec<u8>) -> Parsed {
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    python::read(&Bytes::from(data), 0, tree, &ctx).unwrap()
}

/// A write of `p` through `tree`: its outcome, and its open's work.
fn write(p: &Parsed, tree: &Tree) -> (Result<(), WriteError>, u64) {
    write_under(p, tree, Limits::default())
}

/// [`write`] under `limits`.
fn write_under(p: &Parsed, tree: &Tree, limits: Limits) -> (Result<(), WriteError>, u64) {
    let wctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        rules: WriteRules::UnityPy,
        limits: &limits,
    };
    let meter = Meter::unlimited();
    let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
    let r = super::write(p, tree, &wctx, &mut out);
    (r, meter.decoded())
}

/// An aligned string of `s`.
fn string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend(u32::try_from(s.len()).unwrap().to_le_bytes());
    out.extend(s);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// A write copies a string's bytes into its output: each is the open's
/// work, as a decode counts the bytes it writes.
#[test]
fn a_written_string_counts_its_bytes() {
    let t = tree(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "string", "s"),
    ]);
    let mut data = Vec::new();
    string(&mut data, &vec![b'a'; LONG]);
    let p = read(&t, data);
    let (r, work) = write(&p, &t);
    r.unwrap();
    assert!(work >= LONG as u64, "{work}");
}

/// `K` `pair`s of two strings and an array of `int`s, all named `a`, then a
/// string `a` of 2^16 bytes, the dict's one value for `a`: each sibling is
/// written from that string, each `pair` indexing it twice by character
/// (UnityPy's `value[0]`, `value[1]`) and the array measuring it, which
/// passes over its bytes; the array then fails at its first element (an
/// `int` from a `str`). Each pass counts the string's bytes first.
#[test]
fn a_string_handed_to_pair_and_array_siblings_counts_each_pass() {
    const K: usize = 8;
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    for _ in 0..K {
        rows.extend([
            NodeRow::new(1, "pair", "a"),
            NodeRow::new(2, "string", "first"),
            NodeRow::new(2, "string", "second"),
        ]);
    }
    rows.push(NodeRow::new(1, "string", "a"));
    let t = tree(rows);
    let mut data = Vec::new();
    for _ in 0..K {
        string(&mut data, b"x");
        string(&mut data, b"y");
    }
    string(&mut data, &vec![b'a'; LONG]);
    let p = read(&t, data);
    let (r, work) = write(&p, &t);
    r.unwrap();
    // Two passes a pair, and the copy of the last sibling.
    let long = LONG as u64;
    assert!(work >= (2 * K as u64 + 1) * long, "{work}");

    let array = tree(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "a"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "int", "data"),
        NodeRow::new(1, "string", "a"),
    ]);
    let mut data = 0_u32.to_le_bytes().to_vec();
    string(&mut data, &vec![b'a'; LONG]);
    let p = read(&array, data);
    let (r, work) = write(&p, &array);
    assert!(matches!(r, Err(WriteError::TypeMismatch { .. })), "{r:?}");
    assert!(work >= long, "{work}");
}

/// A write of a value that is not a dict through a class finds each child
/// by `getattr` on its cleaned name: a child named by 2^16 bytes is cleaned
/// in the steps a short name takes, and the write fails at it as UnityPy's
/// does (no such attribute).
#[test]
fn a_long_child_name_is_cleaned_from_its_first_bytes() {
    let read_with = tree(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "int", "a"),
    ]);
    let p = read(&read_with, 5_i32.to_le_bytes().to_vec());
    let mut steps = Vec::new();
    for len in [8, LONG] {
        let name = "n".repeat(len);
        let written_with = tree(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "X", "a"),
            NodeRow::new(2, "int", name.as_str()),
        ]);
        STEPS.with(|s| s.set(0));
        let (r, _) = write(&p, &written_with);
        assert!(matches!(r, Err(WriteError::TypeMismatch { .. })), "{r:?}");
        steps.push(STEPS.with(std::cell::Cell::get));
    }
    assert!(steps[1] <= steps[0] + 64, "{steps:?}");
}

/// Review round 7's sweep: a write of a dict read through a class whose
/// children repeat a name kept each name's position in a table it counted
/// and cleared by two passes over the children before it wrote the first.
/// The table now grows as the children find their entries, so a write
/// refused at the first child (past `max_values_per_object`, 0 here: the
/// first child's name repeats, so its value counts against the bound)
/// takes the same steps and work for 4 children as for 2^16.
#[test]
fn a_write_refused_at_a_class_first_child_takes_no_pass_over_the_rest() {
    let run = |n: usize| {
        let mut rows = vec![NodeRow::new(0, "Base", "Base")];
        for i in 0..n {
            let name = if i + 1 == n {
                "c0".to_owned()
            } else {
                format!("c{i}")
            };
            rows.push(NodeRow::new(1, "int", name.as_str()));
        }
        let t = tree(rows);
        let p = read(
            &t,
            (0..n)
                .flat_map(|i| u32::try_from(i).unwrap().to_le_bytes())
                .collect(),
        );
        let limits = Limits {
            max_values_per_object: 0,
            ..Limits::default()
        };
        STEPS.with(|s| s.set(0));
        let (r, work) = write_under(&p, &t, limits);
        assert!(
            matches!(&r, Err(WriteError::Limit(l)) if l.kind == LimitKind::ValuesPerObject),
            "{n}: {r:?}"
        );
        (STEPS.with(std::cell::Cell::get), work)
    };
    assert_eq!(run(LONG), run(4), "2^16 children against 4");
}
