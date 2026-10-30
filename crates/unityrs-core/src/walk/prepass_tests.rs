//! Review round 6 (claude B, HIGH): a read that fails at a class's first
//! child had already passed over all of its children to size its dict, so
//! K objects over one class of `k` children, each failing at its first
//! child, cost K x `k` steps that nothing counted (the reviewer's input:
//! 65,536 objects of one byte over a class of 2^20 children). A
//! class whose children neither repeat a name nor hold a registry now sizes
//! its dict without a pass, in [`STEPS`] that do not grow with its
//! children; any other class, and a `ReferencedObject`, counts its passes as
//! the open's work before it makes them (`tests/walk_prepass.rs` stops K
//! of them at the open's allowance).

use std::sync::Arc;

use bytes::Bytes;

use super::dict::{STEPS, VISITS};
use super::{ReadCtx, ReadDetail, ReadOptions, RefTypes, STEP_WORK, boost, python};
use crate::limits::{Limits, Meter};
use crate::stream::Endian;
use crate::typetree::{NodeList, NodeRow, Tree};

/// What the class's children are.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// `n` ints, every name its own.
    Plain,
    /// `n` ints, the last named as the first.
    Dup,
    /// `n` ints, then a `ManagedReferencesRegistry`.
    Registry,
    /// One `ReferencedObject` of `n` ints and its data.
    Referenced,
}

fn wide(n: usize, shape: Shape) -> Arc<Tree> {
    let mut rows = vec![NodeRow::new(0, "Base", "Base")];
    let level = if matches!(shape, Shape::Referenced) {
        rows.push(NodeRow::new(1, "ReferencedObject", "r"));
        2
    } else {
        1
    };
    for i in 0..n {
        let name = if matches!(shape, Shape::Dup) && i + 1 == n {
            "c0".to_owned()
        } else {
            format!("c{i}")
        };
        rows.push(NodeRow::new(level, "int", name.as_str()));
    }
    match shape {
        Shape::Registry => rows.push(NodeRow::new(1, "ManagedReferencesRegistry", "reg")),
        Shape::Referenced => rows.push(NodeRow::new(2, "ReferencedObjectData", "data")),
        Shape::Plain | Shape::Dup => {}
    }
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// One read of a single byte through `tree`, which fails at the first
/// `int`: the map steps, the loop turns and the open's work it took.
fn failing_read(reader: &str, tree: &Arc<Tree>) -> (u64, u64, u64) {
    let data = Bytes::from_static(&[7]);
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    STEPS.with(|s| s.set(0));
    VISITS.with(|s| s.set(0));
    let r = match reader {
        "boost" => boost::read(&data, tree, &ctx),
        _ => python::read(&data, 0, tree, &ctx),
    };
    // The read's own failure: the input ends inside the first `int`.
    let e = r.expect_err(reader);
    assert!(
        matches!(e.detail, ReadDetail::Eof { need: 4 }),
        "{reader}: {e:?}"
    );
    (
        STEPS.with(std::cell::Cell::get),
        VISITS.with(std::cell::Cell::get),
        meter.decoded(),
    )
}

const WIDE: usize = 1 << 16;

/// The reviewer's input, one object of it: a read failing at the first of
/// a plain class's children takes the same steps, turns and work for 4
/// children as for 2^16, in both readers.
#[test]
fn a_read_failing_at_a_plain_class_first_child_takes_no_pass_over_the_rest() {
    for reader in ["boost", "python"] {
        let few = failing_read(reader, &wide(4, Shape::Plain));
        let many = failing_read(reader, &wide(WIDE, Shape::Plain));
        assert_eq!(many, few, "{reader}: 2^16 children against 4");
        assert!(few.0 <= 4 && few.1 == 1, "{reader}: {few:?}");
    }
}

/// A class whose children repeat a name or hold a registry, and a
/// `ReferencedObject`, still pass over their children before the first is
/// read: every step of those passes is counted as the open's work first,
/// in both readers.
#[test]
fn a_pass_over_a_class_children_is_counted_before_it_is_made() {
    let n = u64::try_from(WIDE).unwrap();
    for shape in [Shape::Dup, Shape::Registry, Shape::Referenced] {
        for reader in ["boost", "python"] {
            let (steps, visits, work) = failing_read(reader, &wide(WIDE, shape));
            assert!(steps >= n, "{reader} {shape:?}: {steps} steps");
            assert!(visits <= 2, "{reader} {shape:?}: {visits} turns");
            assert!(
                work >= steps * STEP_WORK,
                "{reader} {shape:?}: {work} work for {steps} steps"
            );
        }
    }
}

/// Review round 6's sweep: the pure-Python reader and the writer ask a
/// node's kind at every visit, and in a tree some type name of which holds a
/// NUL the kind was the whole name's (`py_kind_of`), compared at every visit:
/// `n` elements of a type named by `L` bytes cost `n` x `L`. The kinds are
/// now found once, when the tree compiles: a read and a write of 1,024
/// elements whose type is named by 2^16 bytes take [`STEPS`] linear in the
/// elements.
#[test]
fn a_long_type_name_is_read_once_per_compile_not_once_per_visit() {
    const N: usize = 1024;
    let long = format!("S\0{}", "x".repeat(1 << 16));
    let rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "vector", "v"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, long.as_str(), "data"),
        NodeRow::new(4, "int", "x"),
    ];
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let mut data = u32::try_from(N).unwrap().to_le_bytes().to_vec();
    for i in 0..N {
        data.extend(u32::try_from(i).unwrap().to_le_bytes());
    }
    let data = Bytes::from(data);
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    let bound = 16 * u64::try_from(N).unwrap();
    STEPS.with(|s| s.set(0));
    let p = python::read(&data, 0, &tree, &ctx).unwrap();
    let read = STEPS.with(std::cell::Cell::get);
    assert!(read <= bound, "the read: {read} steps for {N} elements");
    let limits = Limits::default();
    let wctx = super::WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        rules: super::WriteRules::UnityPy,
        limits: &limits,
    };
    let mut out =
        crate::budget::DecodeBuf::new(&meter, 0, u64::MAX, crate::error::LimitKind::InputMemory)
            .unwrap();
    STEPS.with(|s| s.set(0));
    super::write::write(&p, &tree, &wctx, &mut out).unwrap();
    let write = STEPS.with(std::cell::Cell::get);
    assert!(write <= bound, "the write: {write} steps for {N} elements");
    assert_eq!(out.as_slice(), &data[..]);
}

/// Review round 6's sweep: a `ReferencedObject`'s dict reserves its
/// distinct names but its data's, and grew by exactly one entry at each
/// name it had not reserved (the pure-Python reader reserved none), so a
/// growth could copy every entry before it, `n`²/2 in all, unless the
/// allocator grew it in place. Of `n` data children of distinct names, each
/// resolving to a ref type's empty tree, both readers now read in [`STEPS`]
/// linear in `n`, counting what each growth may copy.
#[test]
fn a_referenced_object_of_many_data_children_grows_in_linear_steps() {
    const N: usize = 1 << 14;
    let mut rows = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "ReferencedObject", "r"),
        NodeRow::new(2, "ReferencedManagedType", "type"),
    ];
    for name in ["class", "ns", "asm"] {
        rows.push(NodeRow::new(3, "string", name));
    }
    let names: Vec<String> = (0..N).map(|i| format!("d{i}")).collect();
    for name in &names {
        rows.push(NodeRow::new(2, "ReferencedObjectData", name.as_str()));
    }
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let mut data = Vec::new();
    for s in [&b"C"[..], b"", b""] {
        data.extend(u32::try_from(s.len()).unwrap().to_le_bytes());
        data.extend(s);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    let data = Bytes::from(data);
    let empty = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Empty", "Base")]).unwrap());
    let refs = [crate::files::SerializedType::ref_type(
        114,
        crate::raw::RawStr::from("C"),
        crate::raw::RawStr::from(""),
        crate::raw::RawStr::from(""),
        Some(empty),
    )];
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&refs),
        options: &options,
        meter: &meter,
    };
    for reader in ["boost", "python"] {
        STEPS.with(|s| s.set(0));
        let p = match reader {
            "boost" => boost::read(&data, &tree, &ctx),
            _ => python::read(&data, 0, &tree, &ctx),
        }
        .unwrap_or_else(|e| panic!("{reader}: {e}"));
        let steps = STEPS.with(std::cell::Cell::get);
        assert!(
            steps <= 16 * u64::try_from(N).unwrap(),
            "{reader}: {steps} steps for {N} data children"
        );
        let crate::value::Value::Dict(root) = p.value() else {
            panic!("{reader}");
        };
        let Some((_, crate::value::Value::Dict(r))) = root.entries().first() else {
            panic!("{reader}");
        };
        assert_eq!(r.len(), N + 1, "{reader}: `type` and every data child");
    }
}
