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
