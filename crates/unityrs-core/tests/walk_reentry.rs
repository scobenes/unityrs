//! Reads that hold many values open at once, each held to what it has read
//! and a few values a level.
//!
//! Review round 7 (claude B, MEDIUM): a class's dict reserved one entry a
//! child before it read any, and a `ReferencedObject` re-entering its ref
//! type opened that type's root dict again at each level, so every level
//! held a reservation the width of the type while the next was read. The
//! reviewer's object, a few kilobytes of data 250 levels deep into a type
//! of 750,000 `int`s, charged gigabytes and was refused with `InputMemory`
//! where UnityPy raised `EOFError`. A dict now grows as its children are read
//! (`walk::DICT_FIRST` entries at first). The sweep found the same in
//! vectors: each one open reserved up to `max_prealloc` elements of its
//! own before their bytes were read, so vectors nested `d` deep held `d`
//! times that; the vectors a read holds open now reserve at most that
//! together.
//!
//! `tests/fixtures/walk/reentry.json` (`tools/fixtures/walk.py`, its
//! `walk_reentry.py`) holds the reviewer's scene scaled to 64 levels into a
//! type of 2^16 `int`s, and 16 vectors nested, each counting 2^16 elements,
//! with UnityPy's outcome from both readers. Each read must give it under a
//! meter a fraction of what the old reservations needed.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_reentry

use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, DICT_FIRST, ReadCtx, ReadDetail, ReadError, ReadErrorKind, ReadOptions, RefTypes,
};
use unityrs_core::{Bytes, Parsed, RawStr, Value};

/// The meter a re-entry read is given, per level of the scene: room for the
/// three dicts a level opens (its type's root, its `inner` and `inner`'s
/// `type`) at [`DICT_FIRST`] entries each, with room to spare
/// (`peak_measured` prints 1,152 bytes a level).
const CEILING: u64 = 4 << 10;

fn cases(recipe: &str) -> Vec<Json> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/walk/reentry.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let Json::Array(cases) = serde_json::from_str(&text).unwrap() else {
        panic!("reentry.json is not a list");
    };
    let cases: Vec<Json> = cases
        .into_iter()
        .filter(|c| c["recipe"] == recipe)
        .collect();
    assert!(!cases.is_empty(), "no {recipe} case");
    cases
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn list(rows: Vec<NodeRow>) -> Arc<NodeList> {
    Arc::new(NodeList::trusted(rows).unwrap())
}

/// `[[level, type, name, meta], ...]` as rows.
fn rows(spec: &Json) -> Vec<NodeRow> {
    spec.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
        })
        .collect()
}

/// The ref type `R` from its recipe (`walk_reentry.py`'s `ref_type`).
fn ref_type(ints: usize, dup: bool) -> SerializedType {
    let mut r = vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "ReferencedObject", "inner"),
        NodeRow::new(2, "SInt64", "rid"),
        NodeRow::new(2, "ReferencedManagedType", "type"),
        NodeRow::new(3, "string", "class"),
        NodeRow::new(3, "string", "ns"),
        NodeRow::new(3, "string", "asm"),
        NodeRow::new(2, "ReferencedObjectData", "data"),
    ];
    for i in 0..ints {
        let name = if dup && i + 1 == ints {
            "c0".to_owned()
        } else {
            format!("c{i}")
        };
        r.push(NodeRow::new(1, "int", name.as_str()));
    }
    SerializedType::ref_type(
        114,
        RawStr::from("R"),
        RawStr::from("N"),
        RawStr::from("A"),
        Some(list(r)),
    )
}

/// A case: its name, the object's tree and data, its ref types and
/// UnityPy's outcome from each reader.
struct Case {
    name: String,
    tree: Arc<Tree>,
    data: Bytes,
    refs: Vec<SerializedType>,
    want: [(&'static str, Json); 2],
}

fn n(c: &Json, key: &str) -> usize {
    usize::try_from(c[key].as_u64().unwrap()).unwrap()
}

fn case(c: &Json) -> Case {
    let (data, refs) = if c["recipe"] == "ref" {
        (
            unhex(c["hex"].as_str().unwrap()),
            vec![ref_type(n(c, "ints"), c["dup"].as_bool().unwrap())],
        )
    } else {
        // `levels` counts of `count`, then 4 bytes short of `count` `int`s.
        let count = n(c, "count");
        let mut data = Vec::new();
        for _ in 0..n(c, "levels") {
            data.extend(i32::try_from(count).unwrap().to_le_bytes());
        }
        data.resize(data.len() + 4 * count - 4, 0);
        (data, Vec::new())
    };
    Case {
        name: c["name"].as_str().unwrap().to_owned(),
        tree: Tree::compile(list(rows(&c["nodes"])), &Limits::default()).unwrap(),
        data: Bytes::from(data),
        refs,
        want: [
            ("boost", c["boost"].clone()),
            ("python", c["python"].clone()),
        ],
    }
}

/// Reads `case`'s object with `reader` on `meter`.
fn read(case: &Case, reader: &str, meter: &Arc<Meter>) -> Result<Parsed, ReadError> {
    let options = ReadOptions::default();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&case.refs),
        options: &options,
        meter,
    };
    match reader {
        "boost" => walk::boost::read(&case.data, &case.tree, &ctx),
        _ => walk::python::read(&case.data, 0, &case.tree, &ctx),
    }
}

fn kind(k: ReadErrorKind) -> &'static str {
    match k {
        ReadErrorKind::OutOfBounds => "out_of_bounds",
        ReadErrorKind::NegativeLength => "negative_length",
        ReadErrorKind::RefType => "ref_type",
        ReadErrorKind::Other => "other",
    }
}

/// `r` is UnityPy's outcome `want`: its kind, and for its exception a read
/// past the end of the bytes (`EOFError` from UnityPyBoost, `struct.error`
/// from the pure-Python reader), not a refused charge.
fn gives(r: &Result<Parsed, ReadError>, want: &Json) -> Result<(), String> {
    let e = match r {
        Ok(p) => return Err(format!("read {} bytes", p.bytes_read)),
        Err(e) => e,
    };
    let class = want["err"].as_str().unwrap();
    if kind(e.kind) == want["kind"]
        && matches!(class, "EOFError" | "error")
        && matches!(e.detail, ReadDetail::Eof { .. })
    {
        Ok(())
    } else {
        Err(format!("{} {e}", kind(e.kind)))
    }
}

/// Each read of `recipe`'s cases gives UnityPy's outcome on an unlimited
/// meter and on one of `ceiling(case)` bytes, and returns every charge.
fn under(recipe: &str, ceiling: impl Fn(&Json) -> u64) {
    for c in &cases(recipe) {
        let case = case(c);
        let limit = ceiling(c);
        for (reader, want) in &case.want {
            let what = format!("{} [{reader}]", case.name);
            let r = read(&case, reader, &Meter::unlimited());
            gives(&r, want).unwrap_or_else(|e| panic!("{what}: {e}, UnityPy gave {want}"));
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            let r = read(&case, reader, &meter);
            gives(&r, want)
                .unwrap_or_else(|e| panic!("{what} under {limit} bytes: {e}, UnityPy gave {want}"));
            assert_eq!(meter.used(), 0, "{what}: the charges are returned");
        }
    }
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

/// The reviewer's scene, scaled: [`CEILING`] bytes a level, less than a
/// 64th of what one entry a child at each level needs.
#[test]
fn a_type_reentered_level_after_level_holds_a_few_entries_a_level() {
    let entry = as_u64(size_of::<(NodeId, Value)>());
    under("ref", |c| {
        let (levels, ints) = (as_u64(n(c, "levels")), as_u64(n(c, "ints")));
        assert!(ints > 64 * as_u64(DICT_FIRST));
        assert!(levels * ints * entry > 64 * levels * CEILING);
        levels * CEILING
    });
}

/// Vectors nested 16 deep, each counting 2^16 elements, whose bytes run
/// out in the innermost: under a meter of 3 x 2^16 values (the outermost
/// vector's first reservation, the innermost's `int`s and a growth of
/// them, with room to spare), where 16 first reservations of 2^16 values
/// each need more than 4 times that.
#[test]
fn vectors_nested_reserve_ahead_of_their_bytes_once_in_all() {
    let value = as_u64(size_of::<Value>());
    let prealloc = as_u64(Limits::default().max_prealloc);
    under("nested", |c| {
        let (levels, count) = (as_u64(n(c, "levels")), as_u64(n(c, "count")));
        assert_eq!(count, prealloc);
        let ceiling = 3 * count * value;
        assert!((levels - 1) * count * value > 4 * ceiling);
        ceiling
    });
}

/// The least meter, to 1 KiB, each read gives UnityPy's outcome on: what a
/// read of a case holds at its peak (`--ignored --nocapture` prints it).
#[test]
#[ignore = "a measurement: prints each read's peak"]
fn peak_measured() {
    for c in cases("ref").iter().chain(&cases("nested")) {
        let case = case(c);
        for (reader, want) in &case.want {
            let fits = |limit: u64| {
                let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
                gives(&read(&case, reader, &meter), want).is_ok()
            };
            let (mut lo, mut hi) = (0_u64, 1_u64 << 34);
            assert!(fits(hi));
            while hi - lo > 1024 {
                let mid = lo + (hi - lo) / 2;
                if fits(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let levels = as_u64(n(c, "levels"));
            println!(
                "{} [{reader}]: {hi} bytes at its peak, {} a level",
                case.name,
                hi / levels
            );
        }
    }
}
