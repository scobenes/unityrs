//! UnityPyBoost's reader against vectors its recursive form wrote: each
//! case's outcome (the bytes read and the value, or the error's kind,
//! offset and detail), on `unityrs_testkit::depth`'s shapes. Small shapes
//! are read whole, cut short at every length, and under every memory
//! bound from none to enough; `depth.json`'s shapes at the deepest each
//! reads and one level deeper, the depth limit's exact cutoff.
//!
//! The vectors are `tests/fixtures/walk/boost_golden.txt`. This file, run
//! with `GOLDEN_WRITE` set to a path, writes them there instead of checking
//! them; they were written by the recursive reader (the iterative one's
//! baseline) with this file unchanged:
//!
//!     GOLDEN_WRITE=/tmp/golden.txt scripts/dev cargo test -p unityrs-core --test walk_golden

use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, SharedNodes, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::depth::{Depth, Row, Wrap, Z_ROWS};

struct Shape {
    depth: Depth,
    tree: Arc<Tree>,
    refs: Vec<SerializedType>,
}

fn list(rows: &[Row]) -> SharedNodes {
    let rows = rows
        .iter()
        .map(|&(level, ty, name, meta)| {
            NodeRow::new(i32::from(level), ty, name).with_meta_flag(meta)
        })
        .collect();
    NodeList::trusted(rows).unwrap().into_shared().unwrap()
}

impl Shape {
    fn new(wraps: &[Wrap]) -> Self {
        let depth = Depth::new(wraps);
        let a = list(&depth.rows);
        let tree = Tree::compile(a.clone(), &Limits::default()).unwrap();
        let ref_type = |class: &str, l: SharedNodes| {
            SerializedType::ref_type(
                114,
                RawStr::from(class),
                RawStr::from("n"),
                RawStr::from("a"),
                Some(l),
            )
        };
        Self {
            depth,
            tree,
            refs: vec![ref_type("A", a), ref_type("Z", list(&Z_ROWS))],
        }
    }

    /// The read of `data`, on a meter of `memory` bytes: its outcome.
    fn outcome(&self, data: &[u8], memory: u64) -> String {
        let options = ReadOptions::default();
        let meter =
            Meter::with_decode_work(memory, u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let ctx = ReadCtx {
            endian: Endian::Little,
            ref_types: RefTypes::List(&self.refs),
            options: &options,
            meter: &meter,
        };
        match walk::boost::read(&Bytes::from(data.to_vec()), &self.tree, &ctx) {
            Ok(p) => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                format!("{:?}", p.value()).hash(&mut h);
                format!("ok {} {:016x}", p.bytes_read, h.finish())
            }
            Err(e) => format!("err {:?} {} {:?}", e.kind, e.at, e.detail),
        }
    }
}

fn wraps_of(j: &Json) -> Vec<Wrap> {
    j.as_array()
        .unwrap()
        .iter()
        .flat_map(|w| {
            let n = usize::try_from(w[1].as_u64().unwrap()).unwrap();
            std::iter::repeat_n(Wrap::of(w[0].as_str().unwrap()).unwrap(), n)
        })
        .collect()
}

/// The vectors' lines.
fn vectors() -> String {
    let mut out = String::new();
    // Small shapes: each kind of level, and all of them, once or twice.
    let kinds = [
        Wrap::Class,
        Wrap::Vector,
        Wrap::Aligned,
        Wrap::ClassArray,
        Wrap::Pair,
        Wrap::PairArray,
    ];
    let mut small: Vec<(String, Vec<Wrap>)> = Vec::new();
    for k in kinds {
        small.push((format!("{k:?}x2"), vec![k, k]));
    }
    small.push(("all".to_owned(), kinds.to_vec()));
    for (name, wraps) in &small {
        let shape = Shape::new(wraps);
        for n in 1..=2 {
            let data = shape.depth.data(n);
            let full = shape.outcome(&data, u64::MAX);
            writeln!(out, "{name} n={n} whole: {full}").unwrap();
            for len in 0..data.len() {
                let got = shape.outcome(&data[..len], u64::MAX);
                writeln!(out, "{name} n={n} cut={len}: {got}").unwrap();
            }
            // The least memory the read takes, and every bound below it in
            // 64 steps, each step's outcome.
            let (mut lo, mut hi) = (0u64, 1u64 << 24);
            while lo < hi {
                let mid = u64::midpoint(lo, hi);
                if shape.outcome(&data, mid).starts_with("ok") {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            writeln!(out, "{name} n={n} least memory: {lo}").unwrap();
            for step in 0..=64 {
                let memory = lo * step / 64;
                let got = shape.outcome(&data, memory);
                writeln!(out, "{name} n={n} memory={memory}: {got}").unwrap();
            }
            for memory in lo.saturating_sub(64)..lo {
                let got = shape.outcome(&data, memory);
                writeln!(out, "{name} n={n} memory={memory}: {got}").unwrap();
            }
        }
    }
    // `depth.json`'s shapes: the deepest each reads, and one past it.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/walk/depth.json"
    );
    let cases: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut seen = Vec::new();
    for c in cases.as_array().unwrap() {
        let name = c["shape"].as_str().unwrap().to_owned();
        if seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        let shape = Shape::new(&wraps_of(&c["wraps"]));
        let ok = |n: usize| {
            shape
                .outcome(&shape.depth.data(n), u64::MAX)
                .starts_with("ok")
        };
        let (mut lo, mut hi) = (1usize, 1usize << 12);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        writeln!(out, "{name} deepest: {lo}").unwrap();
        for n in [lo, lo + 1] {
            let got = shape.outcome(&shape.depth.data(n), u64::MAX);
            writeln!(out, "{name} n={n}: {got}").unwrap();
        }
    }
    out
}

#[test]
fn outcomes_are_the_recursive_readers() {
    let stack = Limits::default().stack_needed();
    let got = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(vectors)
        .unwrap()
        .join()
        .unwrap();
    if let Ok(path) = std::env::var("GOLDEN_WRITE") {
        std::fs::write(path, got).unwrap();
        return;
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/walk/boost_golden.txt"
    );
    let want = std::fs::read_to_string(path).unwrap();
    let bad: Vec<String> = want
        .lines()
        .zip(got.lines())
        .filter(|(w, g)| w != g)
        .take(20)
        .map(|(w, g)| format!("want {w}\n have {g}"))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert_eq!(want.lines().count(), got.lines().count(), "vector count");
}
