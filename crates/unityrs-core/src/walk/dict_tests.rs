//! Review round 8 (claude B, two LOWs): two rules of plan 3.10's linear
//! dicts that no test pinned, so the suite passed with either removed. A
//! finished dict gives back what a repeated name or a skipped child left
//! unfilled (`Dict::finish`'s shrink: "nothing holds spare capacity once
//! the read is done"); and each slot a dict's table of positions grows by
//! is a step of the open's work ([`STEP_WORK`]), counted by both readers
//! (`Dict::set`) and by the writer (`write::Lookup::get`).

use std::sync::Arc;

use bytes::Bytes;

use super::{ReadCtx, ReadOptions, RefTypes, STEP_WORK, WriteCtx, WriteRules, boost, python};
use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::files::SerializedType;
use crate::limits::{Limits, Meter};
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{NodeList, NodeRow, Tree};
use crate::value::{Parsed, Value};

const READERS: [&str; 2] = ["boost", "python"];

fn tree(rows: &[(i32, &str, &str)]) -> Arc<Tree> {
    let rows = rows
        .iter()
        .map(|(level, ty, name)| NodeRow::new(*level, *ty, *name))
        .collect();
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// A class of `int` children, one for each name.
fn class(names: &[String]) -> Arc<Tree> {
    let mut rows = vec![(0, "Base", "Base")];
    rows.extend(names.iter().map(|n| (1, "int", n.as_str())));
    tree(&rows)
}

/// `n` little-endian `int`s, 0 to `n - 1`.
fn ints(n: usize) -> Vec<u8> {
    (0..u32::try_from(n).unwrap())
        .flat_map(u32::to_le_bytes)
        .collect()
}

/// `tree` read over `data` by `reader`, on a meter of its own: the value,
/// and the meter's charge and open's work once the read is done.
fn read(reader: &str, tree: &Arc<Tree>, data: &[u8]) -> (Parsed, u64, u64) {
    read_with(reader, tree, data, &[])
}

/// [`read`] with the file's ref types `refs`.
fn read_with(
    reader: &str,
    tree: &Arc<Tree>,
    data: &[u8],
    refs: &[SerializedType],
) -> (Parsed, u64, u64) {
    let data = Bytes::copy_from_slice(data);
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    let p = match reader {
        "boost" => boost::read(&data, tree, &ctx),
        _ => python::read(&data, 0, tree, &ctx),
    }
    .unwrap_or_else(|e| panic!("{reader}: {e:?}"));
    (p, meter.used(), meter.decoded())
}

/// The length of every dict in `v`, outermost first.
fn lens(v: &Value, out: &mut Vec<usize>) {
    match v {
        Value::Dict(m) => {
            out.push(m.len());
            for (_, v) in m.entries() {
                lens(v, out);
            }
        }
        Value::List(items) => items.iter().for_each(|v| lens(v, out)),
        Value::Tuple(items) => items.iter().for_each(|v| lens(v, out)),
        _ => {}
    }
}

/// Every dict and list in `v` whose capacity is not its length:
/// `(length, capacity)`, outermost first.
fn spare(v: &Value, out: &mut Vec<(usize, usize)>) {
    match v {
        Value::Dict(m) => {
            if m.capacity() != m.len() {
                out.push((m.len(), m.capacity()));
            }
            for (_, v) in m.entries() {
                spare(v, out);
            }
        }
        Value::List(items) => {
            if items.capacity() != items.len() {
                out.push((items.len(), items.capacity()));
            }
            for v in items {
                spare(v, out);
            }
        }
        Value::Tuple(items) => {
            for v in items {
                spare(v, out);
            }
        }
        _ => {}
    }
}

/// The ref types of [`unfilled_cases`]: `C`, with no tree.
fn treeless() -> Vec<SerializedType> {
    vec![SerializedType::ref_type(
        114,
        RawStr::from("C"),
        RawStr::from(""),
        RawStr::from(""),
        None,
    )]
}

/// A `ReferencedManagedType` naming [`treeless`]'s `C`: three aligned
/// strings, `C` and two empty.
fn reference() -> Vec<u8> {
    let mut data = 1_u32.to_le_bytes().to_vec();
    data.extend([b'C', 0, 0, 0]);
    data.extend([0; 8]);
    data
}

/// A tree and the data it reads.
type Read = (Arc<Tree>, Vec<u8>);

/// A list element: its name, its rows (level, type, name) and its bytes.
type Element = (
    &'static str,
    &'static [(i32, &'static str, &'static str)],
    usize,
);

/// Each case: a node whose children set fewer entries than it has, with
/// the data it reads, and a tree whose children set the same entries once
/// each, with its data (so its value, read exactly, is charged as the
/// first's must be).
fn unfilled_cases() -> Vec<(&'static str, Read, Read)> {
    // 40 children of one name: one entry, where the dict reserved 16.
    let one: Vec<String> = vec!["x".to_owned(); 40];
    // 40 children of 20 names, each twice: 20 entries, where the dict grew
    // to 32 at its 17th child.
    let twice: Vec<String> = (0..40).map(|i| format!("c{}", i % 20)).collect();
    let distinct: Vec<String> = (0..20).map(|i| format!("c{i}")).collect();
    // Two registries: the second is skipped once the first is in scope
    // (R13), so one entry, where the dict reserved two.
    let registry = |n: usize| {
        let mut rows = Vec::new();
        rows.push((0, "Base", "Base"));
        for name in ["r1", "r2"].into_iter().take(n) {
            rows.push((1, "ManagedReferencesRegistry", name));
            rows.push((2, "int", "version"));
        }
        tree(&rows)
    };
    // A `ReferencedObject` whose `type` names a ref type with no tree: its
    // data is skipped, so one entry, where the dict reserved two.
    let referenced = |data: bool| {
        let mut rows = vec![
            (0, "Base", "Base"),
            (1, "ReferencedObject", "r"),
            (2, "ReferencedManagedType", "type"),
            (3, "string", "class"),
            (3, "string", "ns"),
            (3, "string", "asm"),
        ];
        if data {
            rows.push((2, "ReferencedObjectData", "data"));
        }
        tree(&rows)
    };
    vec![
        (
            "one name",
            (class(&one), ints(40)),
            (class(&one[..1]), ints(1)),
        ),
        (
            "each name twice",
            (class(&twice), ints(40)),
            (class(&distinct), ints(20)),
        ),
        (
            "a skipped registry",
            (registry(2), ints(1)),
            (registry(1), ints(1)),
        ),
        (
            "a skipped reference",
            (referenced(true), reference()),
            (referenced(false), reference()),
        ),
    ]
}

/// A dict whose children set fewer entries than it reserved or grew to (a
/// repeated name, a skipped registry, a `ReferencedObject`'s skipped data)
/// ends with no spare entry, in both readers, and is charged as the same
/// entries read into a dict of exactly their number are.
#[test]
fn a_finished_dict_holds_no_spare_entries() {
    for (what, (t, data), (exact, exact_data)) in unfilled_cases() {
        for reader in READERS {
            let refs = treeless();
            let (p, used, _) = read_with(reader, &t, &data, &refs);
            let mut found = Vec::new();
            spare(p.value(), &mut found);
            assert_eq!(found, [], "{reader} {what}: (length, capacity)");
            let (q, exact_used, _) = read_with(reader, &exact, &exact_data, &refs);
            let (mut got, mut want) = (Vec::new(), Vec::new());
            lens(p.value(), &mut got);
            lens(q.value(), &mut want);
            assert_eq!(got, want, "{reader} {what}: each dict's entries");
            assert_eq!(used, exact_used, "{reader} {what}: the value's charge");
        }
    }
}

/// The same rule for lists (plan 3.10, "Bounds"): a list read past its
/// first reservation (`Limits::max_prealloc` elements, 2^16) grows
/// geometrically but never past its length, so it ends with no spare item,
/// in each reader's list loops (a string, a `pair`, a class; the element
/// aligned or not).
#[test]
fn a_list_read_past_its_first_reservation_holds_no_spare_item() {
    let count = Limits::default().max_prealloc + 1;
    let elements: [Element; 3] = [
        ("string", &[(3, "string", "data")], 4),
        (
            "pair",
            &[
                (3, "pair", "data"),
                (4, "int", "first"),
                (4, "int", "second"),
            ],
            8,
        ),
        ("class", &[(3, "X", "data"), (4, "int", "a")], 4),
    ];
    for (what, element, size) in elements {
        for flag in [0, 0x4000] {
            let mut rows = vec![
                NodeRow::new(0, "Base", "Base"),
                NodeRow::new(1, "vector", "v"),
                NodeRow::new(2, "Array", "Array"),
                NodeRow::new(3, "int", "size"),
            ];
            // The element's own flag: aligned, the pure-Python reader
            // reads it through `read_value_array`.
            rows.extend(element.iter().enumerate().map(|(i, (l, t, n))| {
                NodeRow::new(*l, *t, *n).with_meta_flag(if i == 0 { flag } else { 0 })
            }));
            let t = Tree::compile(
                Arc::new(NodeList::trusted(rows).unwrap()),
                &Limits::default(),
            )
            .unwrap();
            let mut data = u32::try_from(count).unwrap().to_le_bytes().to_vec();
            data.resize(4 + count * size, 0);
            for reader in READERS {
                let (p, _, _) = read(reader, &t, &data);
                let mut found = Vec::new();
                spare(p.value(), &mut found);
                assert_eq!(found, [], "{reader} {what} {flag:#x}: (length, capacity)");
                let Value::Dict(m) = p.value() else {
                    panic!("{reader} {what}: not a dict")
                };
                let items = m.entries().first().map(|(_, v)| v);
                assert!(
                    matches!(items, Some(Value::List(l)) if l.len() == count),
                    "{reader} {what} {flag:#x}: not {count} items"
                );
            }
        }
    }
}

/// `p` written through `tree` under `rules`, on a meter of its own: the
/// open's work.
fn write_work(p: &Parsed, tree: &Tree, rules: WriteRules) -> u64 {
    let limits = Limits::default();
    let wctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        rules,
        limits: &limits,
    };
    let meter = Meter::unlimited();
    let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
    super::write::write(p, tree, &wctx, &mut out).unwrap();
    meter.decoded()
}

/// A class of `N` children whose last repeats the first's name keeps a
/// table of `N - 1` slots, one a distinct name, in each reader's dict and
/// in the writer's lookup; a class of `N` distinct names keeps none. Each
/// slot is [`STEP_WORK`] of the open's work, so the first costs exactly
/// `(N - 1) x STEP_WORK` more than the second: the same values are read,
/// and written, in both.
#[test]
fn each_slot_a_dict_table_grows_by_is_a_step_of_the_opens_work() {
    const N: usize = 1024;
    let distinct: Vec<String> = (0..N).map(|i| format!("c{i}")).collect();
    let mut repeated = distinct.clone();
    repeated[N - 1] = "c0".to_owned();
    let (plain, dup) = (class(&distinct), class(&repeated));
    let data = ints(N);
    let slots = u64::try_from(N - 1).unwrap() * STEP_WORK;
    for reader in READERS {
        let (p, _, plain_read) = read(reader, &plain, &data);
        let (d, _, dup_read) = read(reader, &dup, &data);
        assert_eq!(
            dup_read.checked_sub(plain_read),
            Some(slots),
            "{reader}: the read's work, {dup_read} against {plain_read}"
        );
        for rules in [WriteRules::UnityPy, WriteRules::MirrorReader] {
            let plain_write = write_work(&p, &plain, rules);
            let dup_write = write_work(&d, &dup, rules);
            assert_eq!(
                dup_write.checked_sub(plain_write),
                Some(slots),
                "{reader} {rules:?}: the write's work, {dup_write} against {plain_write}"
            );
        }
    }
}
