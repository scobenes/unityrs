//! Plan 3.10 and 3.11, linear work: what a hostile tree can make the
//! readers and the writer do again and again for a `ReferencedObject`,
//! counted in map steps ([`STEPS`], never a clock) and bounded linearly in
//! the input (claude X, review round 2: the ref-type scan; claude B, round
//! 2: a dict a repeated name hands many siblings).

use std::sync::Arc;

use bytes::Bytes;

use super::dict::STEPS;
use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::files::SerializedType;
use crate::limits::{Limits, Meter};
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{NodeList, NodeRow, Tree};
use crate::walk::{ReadCtx, ReadOptions, RefTypes, WriteCtx, WriteRules, boost, python};

/// What one reader's read took, and what writing its value back under one
/// rule took, in map steps.
struct Steps {
    what: String,
    read: u64,
    write: u64,
}

/// Reads `data` through `rows` with both readers, and writes each value
/// back under both rules, counting each in map steps. Every read and write
/// must succeed.
fn measure(rows: Vec<NodeRow>, data: &[u8], refs: &[SerializedType]) -> Vec<Steps> {
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
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    let limits = Limits::default();
    let mut out = Vec::new();
    for reader in ["boost", "python"] {
        STEPS.with(|s| s.set(0));
        let p = match reader {
            "boost" => boost::read(&data, &tree, &ctx),
            _ => python::read(&data, 0, &tree, &ctx),
        }
        .unwrap_or_else(|e| panic!("{reader}: {e}"));
        let read = STEPS.with(std::cell::Cell::get);
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let wctx = WriteCtx {
                endian: Endian::Little,
                ref_types: RefTypes::List(refs),
                rules,
                limits: &limits,
            };
            let mut buf = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
            STEPS.with(|s| s.set(0));
            super::write::write(&p, &tree, &wctx, &mut buf)
                .unwrap_or_else(|e| panic!("{reader} {rules:?}: {e}"));
            out.push(Steps {
                what: format!("{reader} read, {rules:?} write"),
                read,
                write: STEPS.with(std::cell::Cell::get),
            });
        }
    }
    out
}

/// One ref type per class name, each with its own one-row tree (an empty
/// class: its data reads as `{}`).
fn ref_types(classes: impl Iterator<Item = String>) -> Vec<SerializedType> {
    classes
        .map(|c| {
            let rows = vec![NodeRow::new(0, "Empty", "Base")];
            SerializedType::ref_type(
                114,
                RawStr::from(c.as_str()),
                RawStr::from(""),
                RawStr::from(""),
                Some(Arc::new(NodeList::trusted(rows).unwrap())),
            )
        })
        .collect()
}

/// An aligned string: its length, its bytes, zeros to a multiple of 4.
fn string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&i32::try_from(s.len()).unwrap().to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    out.resize(out.len().next_multiple_of(4), 0);
}

/// A `ReferencedType` node at `level` of `extra` ints and then `class`,
/// `ns` and `asm` (so a scan for each passes every int first).
fn type_rows(rows: &mut Vec<NodeRow>, level: i32, extra: u32) {
    rows.push(NodeRow::new(level, "ReferencedType", "type"));
    for i in 0..extra {
        rows.push(NodeRow::new(level + 1, "int", format!("f{i}").as_str()));
    }
    for name in ["class", "ns", "asm"] {
        rows.push(NodeRow::new(level + 1, "string", name));
    }
}

/// A type's bytes: `extra` ints, then the class, an empty `ns` and `asm`.
fn type_data(out: &mut Vec<u8>, extra: u32, class: &str) {
    for i in 0..extra {
        out.extend_from_slice(&i.to_le_bytes());
    }
    string(out, class);
    string(out, "");
    string(out, "");
}

fn check(steps: &[Steps], bound: u64, what: &str) {
    for s in steps {
        assert!(
            s.read <= bound && s.write <= bound,
            "{}: {} read and {} write steps for {what} (bound {bound})",
            s.what,
            s.read,
            s.write
        );
    }
}

/// Claude X's review, round 2: `N` references into `N` ref types, the
/// `i`th naming the `i`th, so UnityPy's scan of the list for each costs
/// `N`²/2; and each reference reads through a tree of its own, so the
/// value's table of node lists has `N` + 1 lists to find each in. Both
/// readers and the writer find them in steps linear in `N`.
#[test]
fn references_into_as_many_ref_types_resolve_in_linear_steps() {
    const N: u32 = 1 << 12;
    let refs = ref_types((0..N).map(|i| format!("C{i}")));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "refs"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "ReferencedObject", "data"),
    ];
    type_rows(&mut rows, 4, 0);
    rows.push(NodeRow::new(4, "ReferencedObjectData", "data"));
    let mut data = N.to_le_bytes().to_vec();
    for i in 0..N {
        type_data(&mut data, 0, &format!("C{i}"));
    }
    let steps = measure(rows, &data, &refs);
    check(&steps, 32 * u64::from(N), "N references into N ref types");
}

/// A `ReferencedObject` of `D` data children read once a `type` of `T`
/// entries, whose `class`, `ns` and `asm` come last: each data child
/// resolved the same `type` again, `D` × 3`T` steps; now once.
#[test]
fn a_type_resolves_once_for_all_its_data_children() {
    const D: u32 = 1 << 12;
    const T: u32 = 1 << 12;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "ReferencedObject", "o"),
    ];
    type_rows(&mut rows, 2, T);
    for i in 0..D {
        rows.push(NodeRow::new(
            2,
            "ReferencedObjectData",
            format!("data{i}").as_str(),
        ));
    }
    let mut data = Vec::new();
    type_data(&mut data, T, "C");
    let steps = measure(rows, &data, &refs);
    check(&steps, 16 * u64::from(D + T), "D data children of one type");
}

/// Claude B's review, round 2, through a `ReferencedObject`: `K`
/// same-named siblings over the last one's dict, whose `type` has `N`
/// entries before `class`, `ns` and `asm`. Each earlier sibling finds its
/// `type`'s children in that `type`, and resolves its data through it; each
/// built the `type`'s name table again, and scanned it three times.
#[test]
fn siblings_share_one_referenced_object_in_linear_steps() {
    const K: u32 = 1 << 12;
    const N: u32 = 1 << 12;
    let refs = ref_types(std::iter::once("C".to_owned()));
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    let mut data = Vec::new();
    for _ in 0..K {
        rows.push(NodeRow::new(1, "ReferencedObject", "a"));
        type_rows(&mut rows, 2, 0);
        rows.push(NodeRow::new(2, "ReferencedObjectData", "data"));
        // An empty class: the data is skipped.
        type_data(&mut data, 0, "");
    }
    rows.push(NodeRow::new(1, "ReferencedObject", "a"));
    type_rows(&mut rows, 2, N);
    rows.push(NodeRow::new(2, "ReferencedObjectData", "data"));
    type_data(&mut data, N, "C");
    let steps = measure(rows, &data, &refs);
    check(&steps, 16 * u64::from(K + N), "K siblings sharing one dict");
}
