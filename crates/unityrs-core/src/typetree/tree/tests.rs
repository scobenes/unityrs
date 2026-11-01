use std::sync::Arc;

use super::{KINDS, Kind, Shape, Tree, kind_of, py_kind_of};
use crate::budget::{Reservation, Unbounded};
use crate::error::LimitKind;
use crate::limits::{ChargeError, Limits, Meter, MeteredVec};
use crate::typetree::{NodeId, NodeList, NodeRow, TreeError};

fn list(rows: &[(i32, &str, &str, i32)]) -> Arc<NodeList> {
    let rows = rows
        .iter()
        .map(|(l, t, n, m)| NodeRow::new(*l, *t, *n).with_meta_flag(*m))
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

fn compile(rows: &[(i32, &str, &str, i32)]) -> Arc<Tree> {
    Tree::compile(list(rows), &Limits::default()).unwrap()
}

#[test]
fn kind_of_is_unitypyboosts_table() {
    // UnityPy TypeTreeHelper.cpp:1075-1103, each name exactly.
    assert_eq!(KINDS.len(), 27);
    for (name, kind) in KINDS {
        assert_eq!(kind_of(name), kind, "{name:?}");
        assert_eq!(py_kind_of(name), kind, "{name:?}");
    }
    assert_eq!(kind_of(b"char"), Kind::U8);
    assert_eq!(kind_of(b"Type*"), Kind::U32);
    assert_eq!(kind_of(b"FileSize"), Kind::U64);
    // The `PPtr<` prefix first; anything else is a class.
    assert_eq!(kind_of(b"PPtr<Texture2D>"), Kind::PPtr);
    assert_eq!(kind_of(b"PPtr<"), Kind::PPtr);
    assert_eq!(kind_of(b"PPtr"), Kind::Unknown);
    assert_eq!(kind_of(b"pptr<X>"), Kind::Unknown);
    assert_eq!(kind_of(b"int "), Kind::Unknown);
    assert_eq!(kind_of(b"Int"), Kind::Unknown);
    assert_eq!(kind_of(b""), Kind::Unknown);
    assert_eq!(kind_of(b"\xff"), Kind::Unknown);
    // A C string ends at its NUL (`strcmp`); Python's `str` does not.
    assert_eq!(kind_of(b"int\0x"), Kind::S32);
    assert_eq!(kind_of(b"PP\0tr<"), Kind::Unknown);
    assert_eq!(py_kind_of(b"int\0x"), Kind::Unknown);
    assert_eq!(py_kind_of(b"PPtr<X>"), Kind::Unknown);
}

#[test]
fn shapes_follow_the_readers_switch() {
    let t = compile(&[
        (0, "Base", "Base", 0),
        (1, "float", "f", 0),             // 1
        (1, "string", "s", 0),            // 2: R9, its own kind first
        (2, "Array", "Array", 0),         // 3
        (3, "int", "size", 0),            // 4
        (3, "char", "data", 0),           // 5
        (1, "TypelessData", "t", 0x4000), // 6
        (1, "pair", "p", 0),              // 7
        (2, "int", "first", 0),           // 8
        (2, "int", "second", 0),          // 9
        (1, "pair", "q", 0),              // 10: one child
        (2, "int", "first", 0),           // 11
        (1, "vector", "v", 0),            // 12
        (2, "Array", "Array", 0x4000),    // 13
        (3, "int", "size", 0),            // 14
        (3, "UInt16", "data", 0),         // 15
        (1, "map", "m", 0),               // 16
        (2, "Array", "Array", 0),         // 17
        (3, "int", "size", 0),            // 18
        (3, "Foo", "data", 0),            // 19
        (1, "PPtr<X>", "bad", 0),         // 20
        (2, "Array", "Array", 0),         // 21: one child
        (3, "int", "size", 0),            // 22
        (1, "ReferencedObject", "r", 0),  // 23
        (1, "Foo", "c", 0),               // 24
    ]);
    let shape = |i: u32| t.info(NodeId(i)).unwrap().shape;
    assert_eq!(shape(0), Shape::Class);
    assert_eq!(shape(1), Shape::Prim);
    assert_eq!(shape(2), Shape::Str);
    assert_eq!(shape(6), Shape::Bytes);
    assert!(t.info(NodeId(6)).unwrap().align);
    assert_eq!(shape(7), Shape::Pair);
    assert_eq!(shape(10), Shape::BadPair);
    assert_eq!(
        shape(12),
        Shape::Vector {
            elem: NodeId(15),
            run: true,
            array_align: true
        }
    );
    assert_eq!(
        shape(16),
        Shape::Vector {
            elem: NodeId(19),
            run: false,
            array_align: false
        }
    );
    assert_eq!(shape(20), Shape::BadVector);
    assert_eq!(shape(23), Shape::RefObject);
    assert_eq!(shape(24), Shape::Class);
    assert_eq!(t.info(NodeId(99)), None);
    assert_eq!(t.depth(), 4);
    assert_eq!(t.root_type(), b"Base");
    // The pure-Python kind: a `PPtr<` is any class.
    assert_eq!(t.py_kind(NodeId(20)), Kind::Unknown);
    assert_eq!(t.py_kind(NodeId(1)), Kind::F32);
}

#[test]
fn repeated_names_share_a_key_and_a_slot() {
    let t = compile(&[
        (0, "Base", "Base", 0),
        (1, "int", "a", 0),
        (1, "int", "b", 0),
        (1, "float", "a", 0),
        (1, "ManagedReferencesRegistry", "c", 0),
        (1, "int", "b", 0),
        (2, "int", "b", 0),
    ]);
    let info = |i: u32| *t.info(NodeId(i)).unwrap();
    let keys: Vec<_> = (1..=5).map(|i| info(i).key.0).collect();
    let slots: Vec<_> = (1..=5).map(|i| info(i).slot).collect();
    assert_eq!(keys, [1, 2, 1, 4, 2]);
    assert_eq!(slots, [0, 1, 0, 2, 1]);
    assert!(info(0).dup_names);
    assert!(info(0).has_registry);
    // Every child whose name a sibling shares, the first included.
    let repeated: Vec<_> = (0..=6).map(|i| info(i).repeated).collect();
    assert_eq!(repeated, [false, true, true, true, false, true, false]);
    // A child's own children are keyed among themselves.
    assert_eq!((info(6).key, info(6).slot), (NodeId(6), 0));
    assert!(!info(5).dup_names);
    assert_eq!((info(0).key, info(0).slot), (NodeId(0), 0));
}

#[test]
fn a_wide_class_groups_its_names_without_quadratic_work() {
    // 65,536 children whose names repeat late: key and slot as a dict.
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    let names: Vec<String> = (0..32_768).map(|i| format!("n{i}")).collect();
    for n in names.iter().chain(names.iter().rev()) {
        rows.push(NodeRow::new(1, "int", n.as_str()));
    }
    let t = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let root = t.info(NodeId(0)).unwrap();
    assert!(root.dup_names);
    let first = *t.info(NodeId(1)).unwrap();
    let last = *t.info(NodeId(65_536)).unwrap();
    assert_eq!((first.key, first.slot), (NodeId(1), 0));
    assert_eq!((last.key, last.slot), (NodeId(1), 0));
    let mid = *t.info(NodeId(32_769)).unwrap();
    assert_eq!((mid.key, mid.slot), (NodeId(32_768), 32_767));
}

#[test]
fn min_sizes_bound_a_successful_read() {
    let t = compile(&[
        (0, "Base", "Base", 0),
        (1, "UInt8", "a", 0),  // 1
        (1, "double", "d", 0), // 8
        (1, "string", "s", 0), // 4
        (1, "pair", "p", 0),   // 2 + 4
        (2, "SInt16", "first", 0),
        (2, "TypelessData", "second", 0),
        (1, "vector", "v", 0), // 4
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "double", "data", 0),
        (1, "ManagedReferencesRegistry", "r", 0), // may be skipped
        (2, "int", "version", 0),
        (1, "Empty", "e", 0), // 0
    ]);
    let min = |i: u32| t.info(NodeId(i)).unwrap().min_size;
    assert_eq!(min(1), 1);
    assert_eq!(min(2), 8);
    assert_eq!(min(4), 6);
    assert_eq!(min(7), 4);
    assert_eq!(min(11), 4);
    assert_eq!(min(13), 0);
    assert_eq!(min(0), 1 + 8 + 4 + 6 + 4);
    let r = compile(&[
        (0, "ReferencedObject", "r", 0),
        (1, "SInt64", "rid", 0),
        (1, "ReferencedObjectData", "data", 0),
        (2, "int", "x", 0),
    ]);
    assert_eq!(r.info(NodeId(0)).unwrap().min_size, 8);
    // A malformed `pair` or `Array` takes what the pure-Python reader reads
    // of it, which UnityPyBoost never reads (review round 4: both had 0).
    let bad = compile(&[
        (0, "Base", "Base", 0),
        (1, "pair", "p", 0), // its first two children: 1 + 2
        (2, "UInt8", "a", 0),
        (2, "SInt16", "b", 0),
        (2, "SInt64", "c", 0),
        (1, "vector", "v", 0), // a length
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, "double", "data", 0),
        (3, "double", "extra", 0),
        (1, "Foo", "f", 0), // its registry may be skipped
        (2, "ManagedReferencesRegistry", "r", 0),
        (3, "int", "x", 0),
    ]);
    let min = |i: u32| bad.info(NodeId(i)).unwrap().min_size;
    assert_eq!(bad.info(NodeId(1)).unwrap().shape, Shape::BadPair);
    assert_eq!(bad.info(NodeId(5)).unwrap().shape, Shape::BadVector);
    assert_eq!((min(1), min(5), min(10), min(0)), (3, 4, 0, 7));
}

#[test]
fn a_tree_deeper_than_the_limit_does_not_compile() {
    let chain = |n: i32| {
        let rows = (0..n).map(|l| NodeRow::new(l, "Foo", "f")).collect();
        Arc::new(NodeList::trusted(rows).unwrap())
    };
    let limits = Limits {
        max_tree_depth: 8,
        ..Limits::default()
    };
    assert_eq!(Tree::compile(chain(8), &limits).unwrap().depth(), 8);
    assert_eq!(
        Tree::compile(chain(9), &limits).unwrap_err(),
        TreeError::Limit(crate::LimitError {
            kind: LimitKind::TreeDepth,
            value: 9,
            max: 8
        })
    );
    // A level jump nests one level deeper, whatever the jump.
    let jump = Arc::new(
        NodeList::trusted(vec![NodeRow::new(0, "A", "a"), NodeRow::new(50, "B", "b")]).unwrap(),
    );
    assert_eq!(Tree::compile(jump, &limits).unwrap().depth(), 2);
}

#[test]
fn the_compile_is_charged_to_the_lists_meter() {
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let rows = MeteredVec::from_vec(
        vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "a"),
            NodeRow::new(1, "int", "a"),
        ],
        &meter,
    )
    .unwrap();
    let list = Arc::new(NodeList::from_rows(rows).unwrap());
    let held = meter.used();
    let tree = Tree::compile(list.clone(), &Limits::default()).unwrap();
    assert_eq!(
        meter.used(),
        held + tree.charge().bytes(),
        "only the tree is kept"
    );
    assert!(tree.charge().bytes() >= 3 * std::mem::size_of::<super::NodeInfo>() as u64);
    drop(tree);
    assert_eq!(meter.used(), held);
    // A meter with no room refuses, and holds nothing for the compile.
    let fill = meter.charge(meter.limit() - meter.used()).unwrap();
    let e = Tree::compile(list.clone(), &Limits::default()).unwrap_err();
    assert!(
        matches!(e, TreeError::Charge(ChargeError::Limit(_))),
        "{e:?}"
    );
    drop(fill);
    assert_eq!(meter.used(), held);
    drop(list);
    assert_eq!(meter.used(), 0);
}

/// Review round 2: a run of names that share a length and a hash, which a
/// hostile tree can make of any number of names (FNV-1a multicollisions are
/// cheap to build), was grouped by scanning the run's earlier keys for each
/// member: `k`²/4 name comparisons for `k`/2 names seen twice. Under a hash
/// that makes every name collide, 4,096 names seen twice (the second time
/// in reverse) group in `O(k log k)` comparisons, keyed and slotted as a
/// dict would key them and as the compile's own hash does.
#[test]
fn colliding_names_group_in_k_log_k_comparisons() {
    use std::collections::HashMap;

    use super::{BLANK, COMPARED, NodeInfo, group_by_name, id};

    const N: usize = 1 << 12;
    let names: Vec<String> = (0..N).map(|i| format!("n{i:05}")).collect();
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    for n in names.iter().chain(names.iter().rev()) {
        rows.push(NodeRow::new(1, "int", n.as_str()));
    }
    let list = Arc::new(NodeList::trusted(rows).unwrap());
    let kids = list.children(NodeId(0));
    let mut info: Vec<NodeInfo> = (0..list.len())
        .map(|i| NodeInfo {
            key: id(i),
            ..BLANK
        })
        .collect();
    let mut order = MeteredVec::new(list.meter());
    let mut hashes = MeteredVec::new(list.meter());
    COMPARED.with(|c| c.set(0));
    group_by_name(&list, kids, &mut order, &mut hashes, &mut info, |_| 0).unwrap();
    let compared = COMPARED.with(std::cell::Cell::get);
    let k = 2 * N as u64;
    let bound = 4 * k * u64::from(k.ilog2());
    assert!(
        compared <= bound,
        "{compared} name comparisons for {k} colliding children (bound {bound})"
    );
    // A dict's keys: each name's first sibling, slotted in first-sighting order.
    let mut first: HashMap<&[u8], (NodeId, u32)> = HashMap::new();
    let compiled = Tree::compile(list.clone(), &Limits::default()).unwrap();
    for c in kids {
        let next = u32::try_from(first.len()).unwrap();
        let (key, slot) = *first.entry(list.name(*c)).or_insert((*c, next));
        let got = info[c.index()];
        assert_eq!((got.key, got.slot), (key, slot), "child {c:?}");
        let real = compiled.info(*c).unwrap();
        assert_eq!((real.key, real.slot), (key, slot), "child {c:?}, compiled");
    }
}
