//! Plan 3.10, "Bounds", through the dump: each shape of the core's
//! `tests/fixtures/walk/depth.json` (recorded from UnityPy by
//! `tools/fixtures/walk.py`), in a SerializedFile read by UnityPyBoost's path
//! (`byte_size` set) and by the pure-Python one (`byte_size` 0), at each
//! depth the fixture holds and far past it. The dump at `-j 1` and at `-j 8`
//! is the same, every worker's stack holds the deepest of them, and each
//! object's L2 and L3 outcome is UnityPy's, or the depth limit where UnityPy
//! reads deeper (`PARITY.md` section 19.3, "value depth").
//!
//!     scripts/dev cargo test -p unityrs-dump --test deep_values

mod common;

use std::sync::Arc;

use common::Tmp;
use serde_json::Value as Json;
use unityrs_core::FileContext;
use unityrs_dump::run::{RunOptions, run};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, LIFTED_VALUE_DEPTH, Levels};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::depth::{Depth, Row, Wrap, Z_ROWS};
use unityrs_testkit::serialized::{TypeSpec, build, one_object};

fn depth_cases() -> Vec<Json> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../unityrs-core/tests/fixtures/walk/depth.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str::<Json>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn nodes(rows: &[Row]) -> Vec<NodeSpec> {
    rows.iter()
        .map(|&(level, ty, name, meta)| {
            let mut n = NodeSpec::new(level, ty, name);
            n.meta_flag = meta;
            n
        })
        .collect()
}

fn ref_type(class: &str, rows: &[Row]) -> TypeSpec {
    TypeSpec {
        class_id: 114,
        script_type_index: -1,
        nodes: nodes(rows),
        ref_names: Some((class.into(), b"n".to_vec(), b"a".to_vec())),
        ..TypeSpec::default()
    }
}

/// A SerializedFile of one object, an `A` `n` deep; `byte_size` 0 sends it
/// down UnityPy's pure-Python path.
fn file(depth: &Depth, n: usize, python: bool) -> Vec<u8> {
    let mut spec = one_object(1000, nodes(&depth.rows), depth.data(n));
    if python {
        spec.objects[0].byte_size = Some(0);
    }
    spec.ref_types = vec![ref_type("A", &depth.rows), ref_type("Z", &Z_ROWS)];
    build(&spec)
}

fn wraps(spec: &Json) -> Vec<Wrap> {
    spec.as_array()
        .unwrap()
        .iter()
        .flat_map(|w| {
            let n = usize::try_from(w[1].as_u64().unwrap()).unwrap();
            std::iter::repeat_n(Wrap::of(w[0].as_str().unwrap()).unwrap(), n)
        })
        .collect()
}

/// One input: its name, the fixture case whose UnityPy outcome it has, and
/// which reader.
struct Case {
    name: String,
    unitypy: Json,
    reader: &'static str,
    /// No value of the input is deeper ([`deepest`]).
    deepest: u32,
}

/// A depth no read or write of `n` `A`s of `depth` passes, from the rows
/// alone. An `A` nests a level for each level of its rows (the deepest
/// row's, and one for the root), and the data of its `ReferencedObject`,
/// one level below its row at most, is the next `A`; the last holds a `Z`.
/// The pure-Python reader counts UnityPy's frames, a list comprehension
/// more for each array (`read_value_array`), so each `Array` row adds one.
fn deepest(depth: &Depth, n: usize) -> u32 {
    let levels = |rows: &[Row]| rows.iter().map(|r| u32::from(r.0)).max().unwrap() + 1;
    let arrays = depth.rows.iter().filter(|r| r.1 == "Array").count();
    let per_a = levels(&depth.rows) + 1 + u32::try_from(arrays).unwrap();
    u32::try_from(n).unwrap() * per_a + levels(&Z_ROWS)
}

/// Every case at its own depth, and each shape that UnityPy refuses at fewer
/// than 60 `A`s at 60 too (a review's input was 60 of the `aligned` shape),
/// which it refuses as it refuses the shallower one.
fn inputs(dir: &std::path::Path) -> Vec<Case> {
    let mut out = Vec::new();
    let cases = depth_cases();
    for c in &cases {
        let mut runs = vec![(c.clone(), c["n"].as_u64().unwrap())];
        if c["at"] == "first_raise" && c["n"].as_u64() < Some(60) {
            runs.push((c.clone(), 60));
        }
        let depth = Depth::new(&wraps(&c["wraps"]));
        for (c, n) in runs {
            let n_us = usize::try_from(n).unwrap();
            for (reader, python) in [("boost", false), ("python", true)] {
                let name = format!("{}_{n}_{reader}.assets", c["shape"].as_str().unwrap());
                std::fs::write(dir.join(&name), file(&depth, n_us, python)).unwrap();
                out.push(Case {
                    name,
                    unitypy: c.clone(),
                    reader,
                    deepest: deepest(&depth, n_us),
                });
            }
        }
    }
    out
}

fn dump(tmp: &Tmp, out: &str, jobs: usize) -> Vec<u8> {
    dump_with(tmp, out, jobs, DumpOptions::default())
}

fn dump_with(tmp: &Tmp, out: &str, jobs: usize, dump: DumpOptions) -> Vec<u8> {
    let opts = RunOptions {
        selection: Selection {
            inputs: vec![tmp.0.join("in")],
            ..Selection::default()
        },
        output: tmp.0.join("out").join(out),
        dump: DumpOptions {
            levels: Levels::ALL,
            jobs,
            ..dump
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    run(&opts).unwrap();
    std::fs::read(&opts.output).unwrap()
}

/// The dump's records, each L2 value replaced by `null` ([`without_value`]).
fn records(dump: &[u8]) -> Vec<Json> {
    dump.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(&without_value(l)).unwrap())
        .collect()
}

/// The object record of the input named `name`.
fn object<'a>(records: &'a [Json], name: &str) -> &'a Json {
    records
        .iter()
        .find(|r| r["rec"] == "object" && r["file"][0] == name)
        .unwrap_or_else(|| panic!("no object record for {name}"))
}

/// One object's L2 and L3 against UnityPy's outcome, or what differs.
fn check(c: &Case, rec: &Json) -> Option<String> {
    let exact = c.unitypy["at"] == "quarter";
    let want = &c.unitypy[c.reader];
    let err = &rec["l2"]["error"];
    let kind = err["kind"].as_str();
    let depth_limit = kind == Some("other")
        && err["note"]
            .as_str()
            .is_some_and(|n| n.contains("ValueDepth"));
    let what = format!("{}: UnityPy {want}, have l2 {err} l3 {}", c.name, rec["l3"]);
    if want.get("ok").is_none() {
        return (!depth_limit).then_some(what);
    }
    // UnityPyBoost reads the whole object; the pure-Python reader reads
    // `pos` bytes where `byte_size` says 0, a `check_read`.
    let read = match c.reader {
        "boost" => err.is_null(),
        _ => kind == Some("check_read") && err["bytes_read"] == want["pos"],
    };
    if !read {
        return (exact || !depth_limit).then_some(what);
    }
    if c.reader == "python" {
        // A `check_read` object is not re-saved.
        return None;
    }
    let resave = rec["l3"]["resave"].as_str();
    let ok = match c.unitypy["boost_write"]
        .get("equal")
        .and_then(Json::as_bool)
    {
        None => resave == Some("error"),
        Some(eq) => {
            resave == Some(if eq { "equal" } else { "differs" })
                || (!exact && resave == Some("error"))
        }
    };
    (!ok).then_some(what)
}

/// The record with its L2 value replaced by `null`: a value nests deeper
/// than `serde_json` parses (128 levels).
fn without_value(line: &[u8]) -> Vec<u8> {
    const KEY: &[u8] = b",\"value\":";
    let Some(at) = line.windows(KEY.len()).position(|w| w == KEY) else {
        return line.to_vec();
    };
    let start = at + KEY.len();
    let (mut depth, mut in_str, mut escaped, mut end) = (0usize, false, false, start);
    for (i, &b) in line.iter().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'[' | b'{' => depth += 1,
                b']' | b'}' | b',' if depth == 0 => {
                    end = i;
                    break;
                }
                b']' | b'}' => depth -= 1,
                _ => {}
            }
        }
    }
    [&line[..start], b"null", &line[end..]].concat()
}

#[test]
fn deep_values_dump_the_same_at_any_jobs_as_unitypy_or_the_depth_limit() {
    let tmp = Tmp::new("deep");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = inputs(&dir);
    let one = dump(&tmp, "j1.jsonl", 1);
    assert!(dump(&tmp, "j8.jsonl", 8) == one, "-j 8 differs from -j 1");
    let records = records(&one);
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|c| check(c, object(&records, &c.name)))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// `--unbounded ValueDepth`, G3b's check that a depth refusal is the whole
/// difference (`tools/parity/g3b.py`): every object UnityPyBoost reads,
/// those the default depth refuses included, reads whole with the depth
/// lifted, with no L2 error, or past the encoder's own bound
/// (`encode::MAX_DEPTH`, 4,096 levels, a value no reader at the default
/// depth makes) an `encode` one. (The pure-Python reader and the writer,
/// which UnityPy runs in Python frames, raise at Python's 1,000 where the
/// lifted depth reads on: `PARITY.md` section 19.3, "value depth".)
///
/// The lifted dump reads as the dump does at any depth past every input's
/// deepest value ([`deepest`]): a bound no value reaches refuses nothing,
/// and the depth sets nothing else but the stack a worker starts on
/// (`run::worker_stack`, from the options' limits). So the dump runs at that
/// depth, at `-j 1` and `-j 4` alike, and refuses no value for its depth:
/// its workers reserve the stack these inputs need, where the lifted
/// depth's are several times that ([`LIFTED_VALUE_DEPTH`]), so many that a
/// machine that bounds what a process may reserve refused them at `-j 4`
/// (review round 5: in a debug build, after `ulimit -v 6291456`, `cannot
/// start 4 workers`). A lifted dump on its own workers is
/// `lifted_depth_reads_unitypyboosts_reach` (ignored).
#[test]
fn a_dump_with_the_value_depth_lifted_reads_what_unitypyboost_reads() {
    let tmp = Tmp::new("deep-lifted");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = inputs(&dir);
    let lifted = DumpOptions::default()
        .with_unbounded(&[unityrs_core::LimitKind::ValueDepth])
        .unwrap();
    let past = cases.iter().map(|c| c.deepest).max().unwrap() + 1;
    assert_eq!(lifted.context.limits.max_value_depth, LIFTED_VALUE_DEPTH);
    assert!(past <= LIFTED_VALUE_DEPTH, "{past} levels");
    let mut context = FileContext::clone(&DumpOptions::default().context);
    context.limits.max_value_depth = past;
    let at_past = DumpOptions {
        context: Arc::new(context),
        ..DumpOptions::default()
    };
    let one = dump_with(&tmp, "past-j1.jsonl", 1, at_past.clone());
    assert!(
        dump_with(&tmp, "past-j4.jsonl", 4, at_past) == one,
        "-j 4 differs from -j 1"
    );
    assert!(
        !one.windows(b"limit ValueDepth".len())
            .any(|w| w == b"limit ValueDepth"),
        "a value reaches {past} levels: `deepest` is no bound"
    );
    let (default, lifted) = (records(&dump(&tmp, "j1.jsonl", 1)), records(&one));
    let mut refused_by_default = 0;
    for c in cases
        .iter()
        .filter(|c| c.reader == "boost" && c.unitypy["boost"].get("ok").is_some())
    {
        let err = &object(&lifted, &c.name)["l2"]["error"];
        assert!(
            err.is_null() || err["kind"] == "encode",
            "{}: UnityPyBoost reads it; lifted {err}",
            c.name
        );
        let was = &object(&default, &c.name)["l2"]["error"]["note"];
        refused_by_default += usize::from(was.as_str().is_some_and(|n| n.contains("ValueDepth")));
    }
    assert!(refused_by_default > 0, "no case passes the default depth");
}

const PROBE: &str = "UNITYRS_DUMP_STACK_PROBE";

/// The child process `stack_measured` runs: every input dumped on one
/// worker of the stack it names.
#[test]
#[ignore = "run by stack_measured"]
fn stack_probe() {
    let Ok(stack) = std::env::var(PROBE) else {
        return;
    };
    let tmp = Tmp::new("deep-probe");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = inputs(&dir);
    let opts = DumpOptions::default();
    let dump_one = |c: &Case| {
        let data = std::fs::read(dir.join(&c.name)).unwrap();
        unityrs_dump::dump_input(c.name.as_bytes(), data.into(), &opts).text()
    };
    unityrs_dump::run::ordered_on(&cases, 1, stack.parse().unwrap(), &dump_one, &mut |_, _| {
        Ok(())
    })
    .unwrap();
}

/// The least worker stack that dumps every input, to 4 KiB, found by
/// bisection over child processes (`unityrs_testkit::stack`: only a stack
/// overflow says a size is too small, and any other failure fails the
/// test), against [`worker_stack`] at the default limits:
///
///     scripts/dev cargo test -p unityrs-dump --test deep_values -- --ignored --nocapture
///     scripts/dev cargo test -p unityrs-dump --test deep_values --release -- --ignored --nocapture
#[test]
#[ignore = "measures; run with --ignored --nocapture"]
fn stack_measured() {
    let hi = unityrs_testkit::stack::least(64 << 20, "the deepest inputs", |s| {
        unityrs_testkit::stack::fits("stack_probe", PROBE, &s.to_string())
    });
    let have = unityrs_dump::run::worker_stack(&unityrs_core::limits::Limits::default());
    println!("least {hi}, worker_stack {have}");
    assert!(hi <= have);
}

/// `unityrs_dump::LIFTED_VALUE_DEPTH` against UnityPyBoost's reach: each
/// `SHAPE:N` of `UNITYRS_BOOST_REACH`, the deepest `N` UnityPyBoost reads
/// as `tools/fixtures/walk.py --boost-reach` prints it, dumped with the
/// value depth lifted reads past the reader (an `encode` refusal past the
/// encoder's own `MAX_DEPTH` is the encoder's, not the read's). A
/// measurement, since the reach depends on the oracle's stack size:
///
///     R=ref:21823,class:174,vector:346,aligned:354,class_array:262,pair:174,pair_array:232
///     scripts/dev env UNITYRS_BOOST_REACH=$R cargo test --release -p unityrs-dump \
///         --test deep_values -- --ignored --nocapture lifted_depth_reads
///
/// (`R` is the reach `walk.py --boost-reach` printed on an 8 MiB main thread
/// in the dev container; each reads.)
#[test]
#[ignore = "measures; run with --ignored and UNITYRS_BOOST_REACH"]
fn lifted_depth_reads_unitypyboosts_reach() {
    let Ok(reach) = std::env::var("UNITYRS_BOOST_REACH") else {
        return;
    };
    let tmp = Tmp::new("deep-reach");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = depth_cases();
    let mut names = Vec::new();
    for item in reach.split(',') {
        let (shape, n) = item.split_once(':').unwrap();
        let c = cases.iter().find(|c| c["shape"] == shape).unwrap();
        let n: usize = n.parse().unwrap();
        let name = format!("{shape}_{n}_boost.assets");
        let depth = Depth::new(&wraps(&c["wraps"]));
        std::fs::write(dir.join(&name), file(&depth, n, false)).unwrap();
        names.push(name);
    }
    let lifted = DumpOptions::default()
        .with_unbounded(&[unityrs_core::LimitKind::ValueDepth])
        .unwrap();
    let records = records(&dump_with(&tmp, "reach.jsonl", 1, lifted));
    for name in &names {
        let err = &object(&records, name)["l2"]["error"];
        println!("{name}: l2 error {err}");
        assert!(
            err.is_null() || err["kind"] == "encode",
            "{name}: the lifted read stopped: {err}"
        );
    }
}
