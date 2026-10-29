use std::sync::Arc;

use super::{Map, Parsed, Value};
use crate::typetree::{NodeId, NodeList, NodeRow};

#[test]
fn value_stays_small() {
    assert!(
        std::mem::size_of::<Value>() <= 40,
        "{}",
        std::mem::size_of::<Value>()
    );
    assert_eq!(std::mem::size_of::<Map>(), 32);
}

#[test]
fn a_map_can_name_a_list_past_u16() {
    // An object that reads through 65,536 distinct ref-type trees has 65,537
    // lists; the last one's keys must still resolve.
    let n = 65_537_usize;
    let list = Arc::new(
        NodeList::trusted(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "x"),
        ])
        .unwrap(),
    );
    let lists: Vec<Arc<NodeList>> = (0..n).map(|_| list.clone()).collect();
    let tail = super::ListIx::try_from(n - 1).unwrap();
    let inner = Map::from_entries(tail, vec![(NodeId(1), Value::Int(5))]);
    let p = Parsed::for_tests(lists, Value::Dict(inner), 4);
    let Value::Dict(m) = &p.value else { panic!() };
    assert_eq!(m.list(), tail);
    assert_eq!(p.key_name(m, NodeId(1)), b"x");
}

#[test]
// The conversion keeps the test meaningful against a narrower field type.
#[allow(clippy::useless_conversion)]
fn bytes_read_can_be_negative() {
    // UnityPy's pure-Python reader: a TypelessData length of -100 at byte 200
    // ends the read at 104, so bytes_read is -96 (tests/fixtures: walk).
    let list = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
    let p = Parsed::for_tests(
        vec![list],
        Value::Dict(Map::new(0)),
        (-96_i64).try_into().unwrap(),
    );
    assert_eq!(p.bytes_read, -96);
}

#[test]
fn a_parsed_value_holds_its_charge_until_it_drops() {
    use crate::budget::{Counting, Reservation};
    use crate::limits::Meter;
    // A batch budget sees a value a caller keeps, and gets it back on drop.
    let budget = Arc::new(Counting::new(1 << 20));
    let meter = Meter::new(1 << 20, budget.clone(), Reservation::Input);
    let list = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "int", "v")]).unwrap());
    let b = super::ValueBuilder::new(&meter, list).unwrap();
    let mut items = b.vec();
    for i in 0..100 {
        items.push(Value::Int(i)).unwrap();
    }
    let v = b.list(items);
    let p = b.finish(v, 4);
    assert!(budget.used() >= 100 * 40);
    assert_eq!(meter.used(), p.charge().bytes());
    assert_eq!(p.charge().records(), 0, "one permit, merged");
    let kept = vec![p];
    assert!(budget.used() >= 100 * 40, "retained, still booked");
    drop(kept);
    assert_eq!((meter.used(), budget.used()), (0, 0));
}

#[test]
fn the_lists_table_is_charged_and_deduplicated() {
    use crate::budget::{Reservation, Unbounded};
    use crate::limits::{Meter, heap_cost};
    // Round 3 (codex HIGH): `Parsed::lists` was allocated by the caller
    // and charged by nobody.
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
    let mut b = super::ValueBuilder::new(&meter, root).unwrap();
    let refs: Vec<_> = (0..100)
        .map(|_| Arc::new(NodeList::trusted(vec![NodeRow::new(0, "R", "R")]).unwrap()))
        .collect();
    for r in refs.iter().chain(refs.iter()) {
        b.list_index(r).unwrap();
    }
    assert_eq!(b.list_index(&refs[99]).unwrap(), 100);
    let v = b.scalar(Value::Int(0)).unwrap();
    let p = b.finish(v, 0);
    assert_eq!(p.lists().len(), 101, "one per distinct tree");
    let table = 128 * std::mem::size_of::<Arc<NodeList>>();
    assert_eq!(meter.used(), heap_cost(u64::try_from(table).unwrap()));
    drop(p);
    assert_eq!(meter.used(), 0);
}

#[test]
fn a_built_value_keeps_its_charge_when_the_builder_is_dropped() {
    use crate::budget::{Reservation, Unbounded};
    use crate::limits::{Meter, heap_cost};
    use crate::value::PrimRun;
    // Round 5 (Codex): `list` moved its vector's charge into the
    // builder and handed the value out bare, so a 1,000-item list built
    // and kept past a dropped builder was alive with the meter at zero.
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
    let b = super::ValueBuilder::new(&meter, root).unwrap();
    let mut items = b.vec();
    for i in 0..1000 {
        items.push(Value::Int(i)).unwrap();
    }
    let cap = u64::try_from(items.capacity() * std::mem::size_of::<Value>()).unwrap();
    let list = b.list(items);
    let list_charge = list.charge().bytes();
    assert_eq!(
        list_charge,
        heap_cost(cap),
        "the list carries its vector's charge"
    );
    let mut entries = b.vec();
    entries.push_entry(NodeId(1), list).unwrap();
    let mut run = b.vec();
    run.extend_from_slice(&[1_i32, 2, 3]).unwrap();
    let run = b.prims(PrimRun::I32(run)).unwrap();
    let run_charge = run.charge().bytes();
    assert_eq!(
        run_charge,
        heap_cost(12),
        "the run carries its exact allocation's charge"
    );
    entries.push_entry(NodeId(2), run).unwrap();
    let entries_heap =
        u64::try_from(entries.capacity() * std::mem::size_of::<(NodeId, Value)>()).unwrap();
    let map = b.map(0, entries);
    assert_eq!(
        map.charge().bytes(),
        heap_cost(entries_heap) + list_charge + run_charge,
        "the map carries its entries' charge and each child's"
    );
    // The builder holds only its table of node lists (one, the root).
    let table = u64::try_from(std::mem::size_of::<Arc<NodeList>>()).unwrap();
    assert_eq!(meter.used(), map.charge().bytes() + heap_cost(table));
    drop(b);
    assert_eq!(
        meter.used(),
        map.charge().bytes(),
        "the value outlives the builder, charged"
    );
    let Value::Dict(m) = map.value() else {
        panic!("{map:?}")
    };
    assert_eq!(m.len(), 2);
    drop(map);
    assert_eq!(meter.used(), 0);
}

#[test]
fn a_value_with_heap_of_its_own_is_built_only_by_the_builder() {
    use crate::limits::Meter;
    let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
    let b = super::ValueBuilder::new(&Meter::unlimited(), root).unwrap();
    for owns in [
        Value::List(vec![Value::Int(1)]),
        Value::Tuple(vec![Value::Int(1)].into_boxed_slice()),
        Value::Dict(Map::from_entries(0, vec![(NodeId(1), Value::Int(1))])),
        Value::Prims(super::PrimArray::I32(vec![1].into_boxed_slice())),
    ] {
        assert!(b.scalar(owns).is_err());
    }
    for none in [
        Value::Int(1),
        Value::Str("x".into()),
        Value::List(Vec::new()),
        Value::Dict(Map::new(0)),
        Value::Prims(super::PrimArray::U8(bytes::Bytes::from_static(b"ab"))),
    ] {
        assert_eq!(b.scalar(none).unwrap().charge().bytes(), 0);
    }
}

#[test]
fn a_replaced_entry_takes_the_new_values_charge() {
    use crate::budget::{Reservation, Unbounded};
    use crate::limits::Meter;
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
    let b = super::ValueBuilder::new(&meter, root).unwrap();
    let mut entries = b.vec();
    entries
        .push_entry(NodeId(1), b.scalar(Value::Int(1)).unwrap())
        .unwrap();
    let mut items = b.vec();
    items.push(Value::Int(2)).unwrap();
    let list = b.list(items);
    let charged = list.charge().bytes();
    let before = meter.used();
    assert!(entries.set_entry(0, list));
    assert_eq!(meter.used(), before, "the charge moved, nothing new");
    assert!(entries.charge().bytes() >= charged);
    let stray = b.scalar(Value::Int(3)).unwrap();
    assert!(!entries.set_entry(5, stray));
    assert_eq!(entries.as_slice()[0].1, Value::List(vec![Value::Int(2)]));
}

#[test]
fn a_repeated_key_keeps_its_first_position_and_the_last_value() {
    let mut m = Map::new(0);
    m.insert(NodeId(1), Value::Int(1));
    m.insert(NodeId(2), Value::Int(2));
    m.insert(NodeId(1), Value::F32(0x4000_0000));
    assert_eq!(
        m.entries(),
        &[
            (NodeId(1), Value::F32(0x4000_0000)),
            (NodeId(2), Value::Int(2))
        ]
    );
}

#[test]
fn replace_at_keeps_the_position_and_takes_the_value() {
    let mut m = Map::from_entries(
        0,
        vec![(NodeId(1), Value::Int(1)), (NodeId(2), Value::Int(2))],
    );
    assert_eq!(m.replace_at(0, Value::Bool(true)), Ok(()));
    assert_eq!(
        m.entries(),
        &[(NodeId(1), Value::Bool(true)), (NodeId(2), Value::Int(2))]
    );
    assert_eq!(m.replace_at(2, Value::Int(9)), Err(Value::Int(9)));
}

#[test]
fn fields_resolve_through_the_node_list() {
    let list = NodeList::trusted(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "int", "a"),
        NodeRow::new(1, "string", "m_Name"),
    ])
    .unwrap();
    let m = Map::from_entries(
        0,
        vec![
            (NodeId(1), Value::Int(7)),
            (NodeId(2), Value::Str("x".into())),
        ],
    );
    let p = Parsed::for_tests(vec![Arc::new(list)], Value::Dict(m), 12);
    assert_eq!(p.get(&[b"m_Name"]).and_then(Value::as_str), Some("x"));
    assert_eq!(p.get(&[b"a"]).and_then(Value::as_i64), Some(7));
    assert_eq!(p.get(&[b"missing"]), None);
}
