//! Plan 3.10, "Bounds": every recursion an object's bytes drive stops at a
//! counted depth, `max_value_depth`, which refuses every object UnityPy
//! refuses with `RecursionError`, and the deepest each reaches under the
//! default limits fits the stack [`Limits::stack_needed`] names, on a
//! thread of exactly that size.
//!
//! The shapes and UnityPy's outcomes are `tools/fixtures/walk.py`'s
//! `depth.json`: ref type `A` nests one kind of level, or every kind after
//! every other (`mixed`), as deep as one tree may go (`max_tree_depth`)
//! around a `ReferencedObject` whose data is the next `A`, and the chain
//! ends at a `Z`. `unityrs_testkit::depth` builds the same rows and bytes
//! from each case's `wraps` and `n`. Every value read is written back under
//! both [`WriteRules`]: `UnityPy`'s outcome is the fixture's, and
//! `MirrorReader`'s the bytes read, or the depth limit.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_stack
//!
//! `stack_measured` (ignored) finds the least stack each path needs, by
//! running it in a child process per size (a thread that overflows its
//! stack aborts its process; only an overflow says a size is too small, and
//! any other failure fails the test: `unityrs_testkit::stack`), prints
//! each, and fails unless the worst is the figure `Limits::stack_needed`'s
//! docs give for the build:
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_stack -- --ignored --nocapture
//!     scripts/dev cargo test -p unityrs-core --test walk_stack --release -- --ignored --nocapture

use std::sync::Arc;

use serde_json::Value as Json;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ReadCtx, ReadDetail, ReadError, ReadErrorKind, ReadOptions, RefTypes, WriteCtx,
    WriteError, WriteRules,
};
use unityrs_core::{Bytes, LimitKind, Parsed, RawStr};
use unityrs_testkit::depth::{Depth, Row, Wrap, Z_ROWS};

/// A shape: ref type `A`'s tree (also the object's) and its bytes
/// (`unityrs_testkit::depth`).
struct Shape {
    name: String,
    depth: Depth,
    tree: Arc<Tree>,
    refs: Vec<SerializedType>,
}

fn list(rows: &[Row]) -> Arc<NodeList> {
    let rows = rows
        .iter()
        .map(|&(level, ty, name, meta)| {
            NodeRow::new(i32::from(level), ty, name).with_meta_flag(meta)
        })
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

impl Shape {
    /// `wraps` is `[[name, count], ...]`, as `depth.json` holds it.
    fn new(name: &str, wraps: &Json) -> Self {
        let wraps: Vec<Wrap> = wraps
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|w| {
                let n = usize::try_from(w[1].as_u64().unwrap()).unwrap();
                std::iter::repeat_n(Wrap::of(w[0].as_str().unwrap()).unwrap(), n)
            })
            .collect();
        let depth = Depth::new(&wraps);
        let a = list(&depth.rows);
        let tree = Tree::compile(a.clone(), &Limits::default()).unwrap();
        let ref_type = |class: &str, l: Arc<NodeList>| {
            SerializedType::ref_type(
                114,
                RawStr::from(class),
                RawStr::from("n"),
                RawStr::from("a"),
                Some(l),
            )
        };
        Self {
            name: name.to_owned(),
            depth,
            tree,
            refs: vec![ref_type("A", a), ref_type("Z", list(&Z_ROWS))],
        }
    }

    fn data(&self, n: usize) -> Bytes {
        Bytes::from(self.depth.data(n))
    }
}

fn depth_cases() -> Vec<Json> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/walk/depth.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str::<Json>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// Each shape of `depth.json` once.
fn shapes() -> Vec<Shape> {
    let mut out: Vec<Shape> = Vec::new();
    for c in depth_cases() {
        let name = c["shape"].as_str().unwrap();
        if !out.iter().any(|s| s.name == name) {
            out.push(Shape::new(name, &c["wraps"]));
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reader {
    Boost,
    Python,
}

impl Reader {
    const BOTH: [Self; 2] = [Self::Boost, Self::Python];

    fn key(self) -> &'static str {
        match self {
            Self::Boost => "boost",
            Self::Python => "python",
        }
    }
}

fn read(shape: &Shape, reader: Reader, data: &Bytes) -> Result<Parsed, ReadError> {
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&shape.refs),
        options: &options,
        meter: &meter,
    };
    match reader {
        Reader::Boost => walk::boost::read(data, &shape.tree, &ctx),
        Reader::Python => walk::python::read(data, 0, &shape.tree, &ctx),
    }
}

fn write(shape: &Shape, p: &Parsed, rules: WriteRules) -> Result<Vec<u8>, WriteError> {
    let limits = Limits::default();
    let ctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&shape.refs),
        rules,
        limits: &limits,
    };
    let mut out = DecodeBuf::new(&Meter::unlimited(), 0, u64::MAX, LimitKind::InputMemory).unwrap();
    walk::write::write(p, &shape.tree, &ctx, &mut out)?;
    Ok(out.as_slice().to_vec())
}

fn read_too_deep(r: &Result<Parsed, ReadError>) -> bool {
    matches!(r, Err(ReadError {
        kind: ReadErrorKind::Other,
        detail: ReadDetail::Limit(l),
        ..
    }) if l.kind() == LimitKind::ValueDepth)
}

fn write_too_deep(r: &Result<Vec<u8>, WriteError>) -> bool {
    matches!(r, Err(WriteError::Limit(l)) if l.kind() == LimitKind::ValueDepth)
}

/// Runs `f` on a thread of `stack` bytes; a stack overflow aborts the
/// process.
fn on_thread(stack: usize, f: impl FnOnce() + Send) {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(s, f)
            .unwrap()
            .join()
            .unwrap();
    });
}

/// One reader's outcome on one case against UnityPy's, or what differs.
fn check(c: &Json, shape: &Shape, data: &Bytes, reader: Reader) -> Option<String> {
    let what = format!("{} [{}]", c["name"].as_str().unwrap(), reader.key());
    // At a quarter of the depth where Python raises, every outcome is
    // UnityPy's; deeper, unityrs may refuse what UnityPy reads.
    let exact = c["at"] == "quarter";
    let want = &c[reader.key()];
    let got = read(shape, reader, data);
    let p = match (want.get("ok"), &got) {
        (None, _) if read_too_deep(&got) => return None,
        (Some(_), Ok(p)) if want["pos"].as_i64() == Some(p.bytes_read) => p,
        (Some(_), _) if !exact && read_too_deep(&got) => return None,
        _ => return Some(format!("{what}: UnityPy {want}, have {got:?}")),
    };
    let want = &c[format!("{}_write", reader.key())];
    let pos = usize::try_from(p.bytes_read).unwrap();
    let w = write(shape, p, WriteRules::UnityPy);
    let ok = match want.get("equal").and_then(Json::as_bool) {
        None => write_too_deep(&w),
        Some(eq) => {
            w.as_ref().is_ok_and(|b| (b[..] == data[..pos]) == eq) || (!exact && write_too_deep(&w))
        }
    };
    if !ok {
        return Some(format!("{what} write: UnityPy {want}, have {w:?}"));
    }
    // The mirror writes back the bytes the reader read: at a quarter of
    // the depth always, deeper unless the depth limit refuses it.
    let m = write(shape, p, WriteRules::MirrorReader);
    let ok = m.as_ref().is_ok_and(|b| b[..] == data[..pos]) || (!exact && write_too_deep(&m));
    (!ok).then(|| format!("{what} MirrorReader write: the bytes read, have {m:?}"))
}

/// UnityPy's outcome at each depth, from both readers and its writer.
/// Wherever UnityPy raises `RecursionError` (the pure-Python reader, or
/// `write_typetree` re-saving UnityPyBoost's value) unityrs refuses the
/// object with `max_value_depth`. At a quarter of that depth every outcome
/// is UnityPy's. Between, UnityPy reads where unityrs may refuse
/// (`PARITY.md` section 19.3, "value depth").
#[test]
fn depth_outcomes_are_unitypys_or_the_depth_limit() {
    let mut bad = Vec::new();
    on_thread(Limits::default().stack_needed(), || {
        for c in depth_cases() {
            let shape = Shape::new(c["shape"].as_str().unwrap(), &c["wraps"]);
            let data = shape.data(usize::try_from(c["n"].as_u64().unwrap()).unwrap());
            for reader in Reader::BOTH {
                bad.extend(check(&c, &shape, &data, reader));
            }
        }
    });
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The most `A`s `reader` reads before `max_value_depth` refuses one more.
fn deepest(shape: &Shape, reader: Reader) -> usize {
    let limit = Limits::default().max_value_depth as usize;
    let mut n = 1;
    loop {
        let r = read(shape, reader, &shape.data(n + 1));
        if read_too_deep(&r) {
            return n;
        }
        assert!(r.is_ok(), "{} {reader:?} at {}: {r:?}", shape.name, n + 1);
        n += 1;
        assert!(n <= limit, "{} {reader:?}: no depth limit", shape.name);
    }
}

/// What runs on the measured thread.
#[derive(Clone, Copy, Debug)]
enum Path {
    /// A read one `A` past the deepest, which `max_value_depth` refuses.
    Refused(Reader),
    /// The deepest read, then that value's write under the rules (refused
    /// or not) and drop.
    Written(Reader, WriteRules),
}

const PATHS: [Path; 6] = [
    Path::Refused(Reader::Boost),
    Path::Refused(Reader::Python),
    Path::Written(Reader::Boost, WriteRules::UnityPy),
    Path::Written(Reader::Python, WriteRules::UnityPy),
    Path::Written(Reader::Boost, WriteRules::MirrorReader),
    Path::Written(Reader::Python, WriteRules::MirrorReader),
];

fn run(shape: &Shape, path: Path) {
    match path {
        Path::Refused(reader) => {
            let n = deepest(shape, reader);
            let r = read(shape, reader, &shape.data(n + 1));
            assert!(read_too_deep(&r), "{} {path:?}: {r:?}", shape.name);
        }
        Path::Written(reader, rules) => {
            let n = deepest(shape, reader);
            let data = shape.data(n);
            let p = read(shape, reader, &data).unwrap();
            let w = write(shape, &p, rules);
            assert!(
                w.as_ref().is_ok_and(|b| b[..] == data[..]) || write_too_deep(&w),
                "{} {path:?}: {w:?}",
                shape.name
            );
            drop(p);
        }
    }
}

/// Every path of every shape at the default limits, on a thread of
/// [`Limits::stack_needed`] bytes: both readers to `max_value_depth` and
/// past it, the deepest value each reads written under both rules and
/// dropped.
#[test]
fn every_path_fits_the_stack_the_limits_name() {
    on_thread(Limits::default().stack_needed(), || {
        for shape in shapes() {
            for path in PATHS {
                run(&shape, path);
            }
        }
    });
}

const PROBE: &str = "UNITYRS_STACK_PROBE";

/// The child process `stack_measured` runs: one path on a thread of the
/// size it names.
#[test]
#[ignore = "run by stack_measured"]
fn stack_probe() {
    let Ok(spec) = std::env::var(PROBE) else {
        return;
    };
    // A path's name has spaces of its own: `Written(Boost, UnityPy)`.
    let (shape, rest) = spec.split_once(' ').unwrap();
    let (path, stack) = rest.rsplit_once(' ').unwrap();
    let shape = shapes().into_iter().find(|s| s.name == shape).unwrap();
    let path = PATHS
        .into_iter()
        .find(|p| format!("{p:?}") == path)
        .unwrap();
    on_thread(stack.parse().unwrap(), || run(&shape, path));
}

/// `n` with a comma between each group of three digits, as the docs write
/// a byte count.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// The least stack each path needs, to 4 KiB, found by bisection over
/// child processes, each within [`Limits::stack_needed`], and the worst of
/// them the figure [`Limits::stack_needed`]'s docs give for this build
/// (review round 11: they gave one measured before a change grew it).
#[test]
#[ignore = "measures; run with --ignored --nocapture"]
fn stack_measured() {
    let needed = Limits::default().stack_needed();
    let mut worst = 0;
    for shape in shapes() {
        for path in PATHS {
            let hi =
                unityrs_testkit::stack::least(64 << 20, &format!("{} {path:?}", shape.name), |s| {
                    unityrs_testkit::stack::fits(
                        "stack_probe",
                        PROBE,
                        &format!("{} {path:?} {s}", shape.name),
                    )
                });
            println!("{:12} {:36} {hi:>9}", shape.name, format!("{path:?}"));
            worst = worst.max(hi);
        }
    }
    println!("worst {worst}, stack_needed {needed}");
    assert!(worst <= needed);
    let docs =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limits/mod.rs")).unwrap();
    // The doc comment's words, its lines' `///` left out.
    let docs = docs
        .split_whitespace()
        .filter(|w| *w != "///")
        .collect::<Vec<_>>()
        .join(" ");
    let figure = if cfg!(debug_assertions) {
        format!("and {} (", grouped(worst))
    } else {
        format!("at most {} bytes (", grouped(worst))
    };
    assert!(
        docs.contains(&figure),
        "Limits::stack_needed's docs do not give the worst measured, {worst} bytes \
         (`{figure}`): update them"
    );
}
