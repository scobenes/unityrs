//! The dump's limit refusals (`PARITY.md` section 19.3): each is its
//! input's own record, in `tools/parity/g3b.py`'s grammar, and a bound
//! `--unbounded` lifts never lets an input outrun a worker's stack.
//!
//! - An input longer than `max_input_bytes` is refused unread, and one whose
//!   buffer cannot be had is a `crash` record; the run goes on
//!   (`unityrs_dump::read`).
//! - An input's records count toward its memory: past `max_input_memory`
//!   they are its `InputMemory` refusal, and refused by the budget its
//!   `crash` record; the run goes on (`unityrs_dump::input`).
//! - Nested containers: lifted, the container depth is
//!   [`LIFTED_CONTAINER_DEPTH`], past what UnityPy reads, and a nest that
//!   deep dumps whole on the worker stack [`worker_stack`] names, at `-j 1`
//!   and `-j 2`, with or without the reverse pre-pass; one level more is the
//!   `ContainerDepth` refusal. Its container term alone pays for the
//!   nesting: the nest dumps on the least stack of its leaf plus
//!   [`CONTAINER_LEVEL_STACK`] a level.
//!
//! `least` finds the least stack a nest needs, by bisection over child
//! processes (a thread that overflows its stack aborts its process), where
//! only a stack overflow says a size is too small and any other failure
//! fails the test (`unityrs_testkit::stack`); `container_stack_measured`
//! (ignored) prints it at several depths, and what a level costs:
//!
//!     scripts/dev cargo test -p unityrs-dump --test limits
//!     scripts/dev cargo test -p unityrs-dump --test limits -- --ignored --nocapture
//!     scripts/dev cargo test -p unityrs-dump --test limits --release -- --ignored --nocapture

mod common;

use std::sync::Arc;

use common::Tmp;
use serde_json::Value as Json;
use unityrs_core::budget::Counting;
use unityrs_core::limits::Limits;
use unityrs_core::{FileContext, LimitKind};
use unityrs_dump::run::{CONTAINER_LEVEL_STACK, RunOptions, run, worker_stack};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, LIFTED_CONTAINER_DEPTH, Levels};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::depth::{Depth, Row, Wrap, Z_ROWS};
use unityrs_testkit::serialized::{ObjectSpec, TypeSpec, build as build_serialized, one_object};

/// The limit refusals in `text`, as `tools/parity/g3b.py`'s `LIMIT`
/// (`\blimit (\w+): (\d+) > (\d+)`) finds them anywhere in a note.
fn limits_in(text: &str) -> Vec<(String, u64, u64)> {
    fn digits(s: &str) -> Option<(u64, &str)> {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        Some((s.get(..n)?.parse().ok()?, s.get(n..)?))
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for (at, _) in text.match_indices("limit ") {
        if text[..at].chars().next_back().is_some_and(word) {
            continue;
        }
        let rest = &text[at + "limit ".len()..];
        let kind: String = rest.chars().take_while(|c| word(*c)).collect();
        let found = rest[kind.len()..]
            .strip_prefix(": ")
            .and_then(digits)
            .and_then(|(v, r)| r.strip_prefix(" > ").and_then(digits).map(|(m, _)| (v, m)));
        if let (false, Some((v, m))) = (kind.is_empty(), found) {
            out.push((kind, v, m));
        }
    }
    out
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

/// The leaf of a [`nest`]: a SerializedFile of two objects, each a value as
/// deep as the default `max_value_depth` lets one read, the `class` shape of
/// `crates/unityrs-core/tests/fixtures/walk/depth.json` 60 `A`s deep (the
/// shape whose read takes the most stack, `tests/walk_stack.rs`), one read
/// by UnityPyBoost's path and one by the pure-Python reader (`byte_size` 0),
/// so the deepest object read happens at the bottom of the nest.
fn leaf() -> Vec<u8> {
    let depth = Depth::new(&[Wrap::of("Class").unwrap(); 248]);
    let ref_type = |class: &str, rows: &[Row]| TypeSpec {
        class_id: 114,
        script_type_index: -1,
        nodes: nodes(rows),
        ref_names: Some((class.into(), b"n".to_vec(), b"a".to_vec())),
        ..TypeSpec::default()
    };
    let mut spec = one_object(1000, nodes(&depth.rows), depth.data(60));
    let mut python = spec.objects[0].clone();
    python.path_id = 2;
    python.byte_size = Some(0);
    spec.objects.push(python);
    spec.ref_types = vec![ref_type("A", &depth.rows), ref_type("Z", &Z_ROWS)];
    build_serialized(&spec)
}

/// `levels` UnityFS bundles each holding the next as its one entry (stored
/// blocks), around [`leaf`]: the top-level bundle is at container depth 0
/// and its entry at 1, so the innermost bundle is at `levels - 1` and the
/// SerializedFile at `levels`. A container deeper than `max_container_depth`
/// is refused, so `max + 1` levels parse and `max + 2` do not.
fn nest(levels: usize) -> Vec<u8> {
    let mut inner = leaf();
    let mut flags = 4;
    for _ in 0..levels {
        let spec = BundleSpec {
            info_codec: TestCodec::None,
            data_codec: TestCodec::None,
            entries: vec![EntrySpec {
                name: b"n".to_vec(),
                data: inner,
                flags,
            }],
            ..BundleSpec::default()
        };
        inner = build(&spec);
        flags = 0;
    }
    inner
}

/// Dumps `<tmp>/in` to `<tmp>/out/<name>`; returns its records.
fn dump(tmp: &Tmp, name: &str, jobs: usize, dump: DumpOptions) -> Vec<Json> {
    let opts = RunOptions {
        selection: Selection {
            inputs: vec![tmp.0.join("in")],
            ..Selection::default()
        },
        output: tmp.0.join("out").join(name),
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
    std::fs::read(&opts.output)
        .unwrap()
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect()
}

/// The records of the input named `name`.
fn of<'a>(records: &'a [Json], name: &str) -> Vec<&'a Json> {
    records
        .iter()
        .filter(|r| r["file"].get(0).and_then(Json::as_str) == Some(name))
        .collect()
}

/// The one record of a refused input: its error's kind and note.
fn refusal(records: &[Json], name: &str) -> (String, String) {
    let recs = of(records, name);
    assert_eq!(recs.len(), 1, "{name}: {recs:?}");
    let e = &recs[0]["error"];
    (
        e["kind"].as_str().unwrap().to_owned(),
        e["note"].as_str().unwrap().to_owned(),
    )
}

fn with_limits(f: impl FnOnce(&mut Limits)) -> DumpOptions {
    let mut ctx = FileContext::unitypy();
    f(&mut ctx.limits);
    DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    }
}

/// A gzip stream of `n` zero bytes, as detection inflates it.
fn gzip_of_zeros(n: usize) -> Vec<u8> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&vec![0; n]).unwrap();
    let mut out = gz.finish().unwrap();
    out.resize(out.len().max(128), 0);
    out
}

/// Every limit refusal the dump writes is one record, of the kind the
/// format allows it, whose note `tools/parity/g3b.py` reads as one refusal
/// with its value above its bound (`test_g3b.py`'s
/// `a_limit_refusal_is_read_wherever_the_dump_writes_it` reads these
/// forms): detection's inflate bound, the input's and an entry's; the
/// input's length; nesting.
#[test]
fn every_limit_note_is_in_g3bs_grammar() {
    let tmp = Tmp::new("limit-notes");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let gz = gzip_of_zeros(5000);
    std::fs::write(dir.join("gz"), &gz).unwrap();
    let entry = unityrs_testkit::env::bundle(&[("big.gz", gz)]);
    std::fs::write(dir.join("gz-entry"), entry).unwrap();
    let nested = nest(20);
    let long = nested.len() + 1;
    std::fs::write(dir.join("long"), vec![b'x'; long]).unwrap();
    std::fs::write(dir.join("nest"), nested).unwrap();
    let records = dump(
        &tmp,
        "notes.jsonl",
        2,
        with_limits(|l| {
            l.max_inflate = 1000;
            l.max_input_bytes = u64::try_from(long - 1).unwrap();
        }),
    );
    let input_bytes = format!("limit InputBytes: {{}} > {}", long - 1);
    for (name, kind, form) in [
        ("gz", "detect", "detection: limit Inflate: {} > 1000"),
        (
            "gz-entry",
            "parse",
            "entry big.gz: detection: limit Inflate: {} > 1000",
        ),
        ("long", "detect", input_bytes.as_str()),
        ("nest", "parse", "limit ContainerDepth: {} > 16"),
    ] {
        let (k, note) = refusal(&records, name);
        let found = limits_in(&note);
        assert!(
            matches!(found.as_slice(), [(_, v, m)] if v > m),
            "{name}: {note}"
        );
        assert_eq!(
            (k.as_str(), note.as_str()),
            (kind, form.replace("{}", &found[0].1.to_string()).as_str()),
            "{name}"
        );
    }
    // The old detection note named no value, so G3b read no refusal in it.
    assert_eq!(
        limits_in("limit Inflate: a gzip stream inflates past 1000 bytes"),
        []
    );
    assert_eq!(limits_in("unlimit A: 2 > 1"), []);
}

/// A bundle entry's gzip detection draws on its open's decode work
/// (`unityrs_core::detect::check_entry_type`), and the detection that passes
/// it is the open's `DecodeWork` limit, in g3b's grammar like every other
/// refusal: the input's own, not wrapped in the entry's name.
#[test]
fn an_entrys_detection_past_the_opens_work_is_in_g3bs_grammar() {
    let tmp = Tmp::new("detect-work");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let gz = gzip_of_zeros(5000);
    let rows = unityrs_testkit::env::bundle(&[("a.gz", gz.clone()), ("b.gz", gz)]);
    std::fs::write(dir.join("gz-rows"), rows).unwrap();
    // Alone, each detection fits; the second passes the open's allowance.
    let records = dump(
        &tmp,
        "work.jsonl",
        1,
        with_limits(|l| l.max_decode_work = 6000),
    );
    let (kind, note) = refusal(&records, "gz-rows");
    let found = limits_in(&note);
    assert!(
        matches!(found.as_slice(), [(k, v, 6000)] if k == "DecodeWork" && *v > 6000),
        "{note}"
    );
    assert_eq!(
        (kind.as_str(), note.as_str()),
        (
            "parse",
            format!("limit DecodeWork: {} > 6000", found[0].1).as_str()
        )
    );
    // Under the default allowance both entries detect.
    let records = dump(&tmp, "default.jsonl", 1, DumpOptions::default());
    assert!(
        of(&records, "gz-rows")
            .iter()
            .all(|r| r.get("error").is_none_or(Json::is_null)),
        "{records:?}"
    );
}

/// An input longer than `max_input_bytes` is refused from its length,
/// unread, with its own record: `l0` names its length and the sha256 of its
/// bytes (so an index's pin still matches), and the inputs beside it dump
/// as they would alone.
#[test]
fn an_input_past_its_bytes_bound_is_its_own_record() {
    let tmp = Tmp::new("input-bytes");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let long = vec![b'x'; 20_000];
    std::fs::write(dir.join("a-small"), vec![b'y'; 300]).unwrap();
    std::fs::write(dir.join("b-long"), &long).unwrap();
    std::fs::write(dir.join("c-small"), vec![b'z'; 300]).unwrap();
    let bounded = dump(
        &tmp,
        "b.jsonl",
        2,
        with_limits(|l| l.max_input_bytes = 19_999),
    );
    let (kind, note) = refusal(&bounded, "b-long");
    assert_eq!(
        (kind.as_str(), note.as_str()),
        ("detect", "limit InputBytes: 20000 > 19999")
    );
    let rec = of(&bounded, "b-long")[0];
    assert_eq!(rec["l0"]["type"], "error");
    assert_eq!(rec["l0"]["parsed_as"], "error");
    assert_eq!(rec["l0"]["size"], 20_000);
    assert_eq!(rec["l0"]["sha256"], unityrs_dump::json::sha256_hex(&long));
    let whole = dump(&tmp, "w.jsonl", 2, DumpOptions::default());
    for name in ["a-small", "c-small"] {
        assert_eq!(of(&bounded, name), of(&whole, name), "{name}");
    }
    assert_eq!(of(&whole, "b-long")[0]["l0"]["parsed_as"], "raw");
    // Lifted, the bound refuses nothing.
    let lifted = DumpOptions::default()
        .with_unbounded(&[LimitKind::InputBytes])
        .unwrap();
    let lifted = dump(&tmp, "l.jsonl", 1, lifted);
    assert_eq!(of(&lifted, "b-long"), of(&whole, "b-long"));
}

/// An input whose buffer the memory budget refuses (as the allocator would
/// for a file larger than memory) is one `crash` record, the dumper having
/// run out of memory on it, and the run goes on.
#[test]
fn an_input_too_large_to_hold_is_a_crash_record_and_the_run_goes_on() {
    let tmp = Tmp::new("input-memory");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a-small"), vec![b'y'; 30]).unwrap();
    std::fs::write(dir.join("b-huge"), vec![b'x'; 100_000]).unwrap();
    let mut ctx = FileContext::unitypy();
    ctx.budget = Arc::new(Counting::new(20_000));
    let opts = DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    };
    let records = dump(&tmp, "m.jsonl", 1, opts);
    let (kind, note) = refusal(&records, "b-huge");
    assert_eq!(kind, "crash");
    assert!(
        note.starts_with("unityrs ran out of memory reading this input"),
        "{note}"
    );
    assert!(limits_in(&note).is_empty(), "{note}");
    let small = of(&records, "a-small");
    assert_eq!(small.len(), 1);
    assert_eq!(small[0]["l0"]["parsed_as"], "raw");
    assert_eq!(records.last().unwrap()["rec"], "end");
}

/// A SerializedFile of `objects` objects, each 4 data bytes that read as a
/// vector of `count` empty structs: its records are thousands of times its
/// size (the dump writes every struct), its parse of one object at a time
/// a fraction of them.
fn empties(objects: i64, count: i32) -> Vec<u8> {
    empties_of(114, objects, count)
}

/// [`empties`] whose objects are of class `class_id`.
fn empties_of(class_id: i32, objects: i64, count: i32) -> Vec<u8> {
    let tree = vec![
        NodeSpec::new(0, "MyThing", "Base"),
        NodeSpec::new(1, "vector", "v"),
        NodeSpec {
            type_flags: 1,
            ..NodeSpec::new(2, "Array", "Array")
        },
        NodeSpec::new(3, "int", "size").size(4),
        NodeSpec::new(3, "Empty", "data"),
    ];
    let mut spec = one_object(class_id, tree, count.to_le_bytes().to_vec());
    let first = spec.objects[0].clone();
    spec.objects = (1..=objects)
        .map(|path_id| ObjectSpec {
            path_id,
            ..first.clone()
        })
        .collect();
    build_serialized(&spec)
}

/// An input's records count toward its memory with what its parse holds,
/// until the run writes them: records past `max_input_memory` are the
/// input's `InputMemory` refusal, in g3b's grammar, and records the budget
/// (or the allocator) refuses are its `crash` record, as the oracle's
/// `MemoryError` is; either way the inputs beside it dump as they would
/// alone. Uncharged, a 2 KB input's records once took the process down.
#[test]
fn an_inputs_records_count_toward_its_memory_and_the_run_goes_on() {
    const MAX: u64 = 4 << 20;
    let tmp = Tmp::new("records-memory");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let big = empties(400, 1 << 12);
    std::fs::write(dir.join("a-small"), empties(2, 3)).unwrap();
    std::fs::write(dir.join("b-big"), &big).unwrap();
    std::fs::write(dir.join("c-small"), empties(1, 5)).unwrap();
    let whole = dump(&tmp, "whole.jsonl", 1, DumpOptions::default());
    let held: usize = of(&whole, "b-big")
        .iter()
        .map(|r| r.to_string().len())
        .sum();
    // The records are well past the bound below; the parse of one object is not.
    assert!(
        held > 3 * usize::try_from(MAX).unwrap(),
        "the records are {held} bytes"
    );
    assert_eq!(of(&whole, "b-big").len(), 401);

    let bounded = dump(
        &tmp,
        "bounded.jsonl",
        2,
        with_limits(|l| l.max_input_memory = MAX),
    );
    let (kind, note) = refusal(&bounded, "b-big");
    let found = limits_in(&note);
    assert!(
        matches!(found.as_slice(), [(k, v, MAX)] if k == "InputMemory" && *v > MAX),
        "{note}"
    );
    assert_eq!(
        (kind.as_str(), note.as_str()),
        (
            "parse",
            format!("limit InputMemory: {} > {MAX}", found[0].1).as_str()
        )
    );
    let rec = of(&bounded, "b-big")[0];
    assert_eq!(rec["l0"]["type"], "AssetsFile");
    assert_eq!(rec["l0"]["parsed_as"], "error");
    assert_eq!(rec["l0"]["sha256"], unityrs_dump::json::sha256_hex(&big));

    let mut ctx = FileContext::unitypy();
    ctx.budget = Arc::new(Counting::new(MAX));
    let budget = DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    };
    let refused = dump(&tmp, "budget.jsonl", 1, budget);
    let (kind, note) = refusal(&refused, "b-big");
    assert_eq!(kind, "crash");
    assert!(
        note.starts_with("unityrs ran out of memory dumping this input ("),
        "{note}"
    );
    assert!(limits_in(&note).is_empty(), "{note}");
    assert_eq!(of(&refused, "b-big")[0]["l0"]["type"], "AssetsFile");
    for records in [&bounded, &refused] {
        for name in ["a-small", "c-small"] {
            assert_eq!(of(records, name), of(&whole, name), "{name}");
        }
        assert_eq!(records.last().unwrap()["rec"], "end");
    }
}

/// An object's read the budget (or the allocator) refuses is the input's
/// `crash` record, as the oracle's `MemoryError` inside an object's parse
/// is, never the object's L2 outcome: it depends on the machine and on what
/// the other inputs hold, not on the input. So is the AssetBundle read at
/// load, whose refusal reaches the dump inside the load's error.
#[test]
fn an_objects_read_the_budget_refuses_is_the_inputs_crash() {
    let tmp = Tmp::new("object-memory");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a-small"), empties(1, 5)).unwrap();
    // One object whose value is 2^16 structs; its record about 1.3 MB.
    std::fs::write(dir.join("b-wide"), empties(1, 1 << 16)).unwrap();
    // The same as the AssetBundle (class 142) the load reads.
    std::fs::write(dir.join("c-asset-bundle"), empties_of(142, 1, 1 << 16)).unwrap();
    let mut ctx = FileContext::unitypy();
    ctx.budget = Arc::new(Counting::new(1 << 20));
    let opts = DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    };
    let records = dump(&tmp, "o.jsonl", 1, opts);
    for name in ["b-wide", "c-asset-bundle"] {
        let (kind, note) = refusal(&records, name);
        assert_eq!(kind, "crash", "{name}: {note}");
        assert!(
            note.starts_with(
                "unityrs ran out of memory dumping this input (memory budget refused "
            ) && note.ends_with(" bytes for Input)"),
            "{name}: {note}"
        );
    }
    let whole = dump(&tmp, "w.jsonl", 1, DumpOptions::default());
    assert_eq!(of(&records, "a-small"), of(&whole, "a-small"));
    // Unbudgeted, the wide object reads, and the AssetBundle, which has none
    // of an AssetBundle's fields, is a `parse` record: neither for memory.
    let wide = of(&whole, "b-wide");
    assert_eq!(wide.len(), 2);
    assert!(
        wide.iter()
            .all(|r| r["error"].is_null() && r["l2"]["error"].is_null())
    );
    let (kind, note) = refusal(&whole, "c-asset-bundle");
    assert_eq!(kind, "parse", "{note}");
    assert!(!note.contains("memory"), "{note}");
}

/// Plan 3.15 and `PARITY.md` 19.3, "nested containers": with the container
/// depth lifted (`--unbounded ContainerDepth`), a nest whose innermost
/// bundle sits at [`LIFTED_CONTAINER_DEPTH`] dumps whole on the workers'
/// [`worker_stack`], at `-j 1` and `-j 2` and through the reverse pre-pass,
/// and one level deeper, or 3,000 levels, is refused there, never a stack
/// overflow.
#[test]
fn a_lifted_container_depth_dumps_to_its_bound_and_refuses_past_it() {
    let tmp = Tmp::new("lifted-nest");
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let depth = usize::try_from(LIFTED_CONTAINER_DEPTH).unwrap();
    std::fs::write(dir.join("at"), nest(depth + 1)).unwrap();
    std::fs::write(dir.join("past"), nest(depth + 2)).unwrap();
    // Lifted to `u32::MAX`, a nest of a few thousand overflowed the stack.
    std::fs::write(dir.join("past-far"), nest(3000)).unwrap();
    let lifted = DumpOptions::default()
        .with_unbounded(&[LimitKind::ContainerDepth])
        .unwrap();
    assert_eq!(
        lifted.context.limits.max_container_depth,
        LIFTED_CONTAINER_DEPTH
    );
    let one = dump(&tmp, "j1.jsonl", 1, lifted.clone());
    let at = of(&one, "at");
    // The leaf's objects read to the value depth bound, at the nest's foot.
    for obj in &at[depth + 2..] {
        let note = obj["l2"]["error"]["note"].as_str().unwrap_or_default();
        assert!(note.contains("limit ValueDepth: "), "{obj}");
    }
    assert_eq!(
        at.len(),
        depth + 4,
        "every bundle, the SerializedFile, its objects"
    );
    assert!(at.iter().all(|r| r.get("error").is_none_or(Json::is_null)));
    for name in ["past", "past-far"] {
        let (kind, note) = refusal(&one, name);
        assert_eq!(
            (kind.as_str(), note.as_str()),
            (
                "parse",
                format!("limit ContainerDepth: {} > {depth}", depth + 1).as_str()
            ),
            "{name}"
        );
    }
    assert_eq!(dump(&tmp, "j2.jsonl", 2, lifted.clone()), one);
    let reverse = DumpOptions {
        lazy_reverse: true,
        ..lifted
    };
    assert_eq!(dump(&tmp, "rev.jsonl", 2, reverse), one);
}

const PROBE: &str = "UNITYRS_DUMP_NEST_PROBE";

/// The child process [`fits`] runs: a nest of `levels` bundles dumped with
/// the container depth lifted, through the reverse pre-pass or not, on one
/// worker of `stack` bytes.
#[test]
#[ignore = "run by fits"]
fn container_stack_probe() {
    let Ok(spec) = std::env::var(PROBE) else {
        return;
    };
    let [levels, stack, reverse]: [&str; 3] =
        spec.split(' ').collect::<Vec<_>>().try_into().unwrap();
    let data = nest(levels.parse().unwrap());
    let opts = DumpOptions {
        lazy_reverse: reverse == "reverse",
        ..DumpOptions::default()
            .with_unbounded(&[LimitKind::ContainerDepth])
            .unwrap()
    };
    let dump_one = |d: &Vec<u8>| unityrs_dump::dump_input(b"nest", d.clone().into(), &opts);
    unityrs_dump::run::ordered_on(
        &[data],
        1,
        stack.parse().unwrap(),
        &dump_one,
        &mut |_, _| Ok(()),
    )
    .unwrap();
}

/// Whether a nest of `levels` dumps (`mode` "eager" or "reverse") on a
/// worker of `stack` bytes, in a child process: false for a stack overflow
/// only, and any other failure of the child (a panic, an allocation that
/// failed, a kill under memory pressure) fails the test with what it was
/// (`unityrs_testkit::stack::run`; review round 5: every unsuccessful
/// child read as an overflow).
fn fits(levels: u32, stack: usize, mode: &str) -> bool {
    unityrs_testkit::stack::fits(
        "container_stack_probe",
        PROBE,
        &format!("{levels} {stack} {mode}"),
    )
}

/// The least worker stack a nest of `levels` dumps on, to 4 KiB, by
/// bisection over [`fits`].
fn least(levels: u32, mode: &str) -> usize {
    unityrs_testkit::stack::least(256 << 20, &format!("{levels} levels ({mode})"), |s| {
        fits(levels, s, mode)
    })
}

/// [`worker_stack`]'s container term pays for the nesting by itself: the
/// deepest nest [`LIFTED_CONTAINER_DEPTH`] lets parse dumps on the least
/// stack its leaf needs alone plus [`CONTAINER_LEVEL_STACK`] for each level
/// the term counts, eagerly and through the reverse pre-pass. The test of
/// the nest on [`worker_stack`] cannot show it: the doubling of
/// `stack_needed` there held the nest with the term zero.
#[test]
fn the_container_term_pays_for_every_level() {
    let levels = LIFTED_CONTAINER_DEPTH + 1;
    let counted = usize::try_from(levels).unwrap();
    for mode in ["eager", "reverse"] {
        let leaf = least(0, mode);
        let stack = leaf + counted * CONTAINER_LEVEL_STACK;
        assert!(
            fits(levels, stack, mode),
            "{levels} levels ({mode}) overflow {stack} bytes: the leaf's least {leaf} \
             and {CONTAINER_LEVEL_STACK} a level"
        );
    }
}

/// The least worker stack that dumps a nest at several depths, from the
/// leaf alone to the deepest [`LIFTED_CONTAINER_DEPTH`] lets parse and one
/// past it (refused), eagerly and through the reverse pre-pass, to 4 KiB;
/// what each level costs; and [`worker_stack`] with the container depth
/// lifted, which must hold each.
#[test]
#[ignore = "measures; run with --ignored --nocapture"]
fn container_stack_measured() {
    let mut lifted = Limits::default();
    lifted.max_container_depth = LIFTED_CONTAINER_DEPTH;
    let have = worker_stack(&lifted);
    for mode in ["eager", "reverse"] {
        let mut prev: Option<(u32, usize)> = None;
        for levels in [
            0,
            17,
            257,
            513,
            LIFTED_CONTAINER_DEPTH + 1,
            LIFTED_CONTAINER_DEPTH + 2,
        ] {
            let need = least(levels, mode);
            let per = prev.map_or(String::new(), |(l, n)| {
                let per = need.saturating_sub(n) / usize::try_from(levels - l).unwrap();
                format!(", {per} a level since {l}")
            });
            println!("{mode:>7} {levels:>5} levels: least {need}{per}");
            prev = Some((levels, need));
            assert!(
                need <= have,
                "{levels} levels need {need}, worker_stack {have}"
            );
        }
    }
    println!(
        "worker_stack with the container depth lifted: {have}; stack_needed {}",
        Limits::default().stack_needed()
    );
}
