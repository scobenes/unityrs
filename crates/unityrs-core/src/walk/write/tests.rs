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

/// Claude B's review, round 2: `K` same-named siblings over one dict of `N`
/// entries, which the read keyed by the last sibling's children, so each
/// earlier sibling misses the cursor on its only child. Each miss once
/// built the dict's name table again.
#[test]
fn many_siblings_share_one_foreign_dict_in_linear_map_steps() {
    const K: u32 = 1 << 12;
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    for _ in 0..K {
        rows.push(NodeRow::new(1, "Foo", "a"));
        rows.push(NodeRow::new(2, "int", "zz"));
    }
    rows.push(NodeRow::new(1, "Bar", "a"));
    for i in 0..K - 1 {
        rows.push(NodeRow::new(2, "int", format!("c{i}").as_str()));
    }
    rows.push(NodeRow::new(2, "int", "zz"));
    for (reader, steps) in write_steps(rows, &ints(2 * K)) {
        assert!(
            steps <= 4 * u64::from(2 * K),
            "{reader}: {steps} map steps for {K} siblings and {K} entries"
        );
    }
}

/// The same, a level down: each sibling's own child finds its fields in a
/// dict inside the shared one, which every earlier sibling indexed again.
#[test]
fn a_dict_inside_a_shared_dict_is_indexed_once() {
    const K: u32 = 1 << 12;
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    for _ in 0..K {
        rows.push(NodeRow::new(1, "Foo", "a"));
        rows.push(NodeRow::new(2, "X", "x"));
        rows.push(NodeRow::new(3, "int", "zz"));
    }
    rows.push(NodeRow::new(1, "Bar", "a"));
    rows.push(NodeRow::new(2, "Y", "x"));
    for i in 0..K - 1 {
        rows.push(NodeRow::new(3, "int", format!("c{i}").as_str()));
    }
    rows.push(NodeRow::new(3, "int", "zz"));
    for (reader, steps) in write_steps(rows, &ints(2 * K)) {
        assert!(
            steps <= 4 * u64::from(2 * K),
            "{reader}: {steps} map steps for {K} siblings and {K} entries"
        );
    }
}

/// [`crate::walk::WriteError::exceptions`] where no recorded write reaches:
/// an object with no tree raises the read side's exception, its `why`'s
/// ([`crate::provider::NoTree::exception`], `helpers/Tpk.py`), and a bound,
/// a refused charge or a failed tree has no counterpart in UnityPy's writer.
#[test]
fn errors_no_recorded_write_reaches_name_their_exceptions() {
    use crate::LimitError;
    use crate::provider::NoTree;
    use crate::walk::{ParseError, WriteError};

    for why in [
        NoTree::UnknownClass,
        NoTree::NoEntry,
        NoTree::NoReleaseRoot,
        NoTree::NoVersion,
    ] {
        let e = WriteError::NoTypeTree {
            class_id: 1,
            why: Some(why),
        };
        assert_eq!(e.exceptions(), [why.exception()], "{why:?}");
    }
    let limit = LimitError::refused(LimitKind::ValueDepth, 2, 1);
    let charge = Meter::new(
        0,
        Arc::new(crate::budget::Unbounded),
        crate::budget::Reservation::Input,
    )
    .charge(1)
    .unwrap_err();
    for e in [
        WriteError::NoTypeTree {
            class_id: 1,
            why: None,
        },
        WriteError::Limit(limit),
        WriteError::Charge(charge),
        WriteError::Tree(Box::new(ParseError::NoTypeTree {
            class_id: 1,
            why: None,
        })),
    ] {
        assert!(e.exceptions().is_empty(), "{e:?}");
    }
}
