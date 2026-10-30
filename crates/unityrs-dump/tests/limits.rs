//! The dump's limit refusals (`PARITY.md` section 19.3): each is its
//! input's own record, in `tools/parity/g3b.py`'s grammar, and a bound
//! `--unbounded` lifts never lets an input outrun a worker's stack.
//!
//! - An input longer than `max_input_bytes` is refused unread, and one whose
//!   buffer cannot be had is a `crash` record; the run goes on
//!   (`unityrs_dump::read`).
//! - Nested containers: lifted, the container depth is
//!   [`LIFTED_CONTAINER_DEPTH`], past what UnityPy reads, and a nest that
//!   deep dumps whole on the worker stack [`worker_stack`] names, at `-j 1`
//!   and `-j 2`, with or without the reverse pre-pass; one level more is the
//!   `ContainerDepth` refusal.
//!
//! `container_stack_measured` (ignored) finds the least stack such a nest
//! needs, by bisection over child processes (a thread that overflows its
//! stack aborts its process), and prints what a level costs:
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
use unityrs_dump::run::{RunOptions, run, worker_stack};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, LIFTED_CONTAINER_DEPTH, Levels};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::depth::{Depth, Row, Wrap, Z_ROWS};
use unityrs_testkit::serialized::{TypeSpec, build as build_serialized, one_object};

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

/// The child process `container_stack_measured` runs: a nest of `levels`
/// bundles dumped with the container depth lifted, through the reverse
/// pre-pass or not, on one worker of `stack` bytes.
#[test]
#[ignore = "run by container_stack_measured"]
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

/// The least worker stack that dumps a nest at several depths, to the
/// deepest [`LIFTED_CONTAINER_DEPTH`] lets parse and one past it (refused),
/// eagerly and through the reverse pre-pass, to 4 KiB, found by bisection
/// over child processes; what each level costs; and [`worker_stack`] with
/// the container depth lifted, which must hold each.
#[test]
#[ignore = "measures; run with --ignored --nocapture"]
fn container_stack_measured() {
    let exe = std::env::current_exe().unwrap();
    let fits = |levels: u32, stack: usize, mode: &str| {
        std::process::Command::new(&exe)
            .args([
                "--exact",
                "container_stack_probe",
                "--ignored",
                "--test-threads=1",
            ])
            .env(PROBE, format!("{levels} {stack} {mode}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let least = |levels: u32, mode: &str| {
        let (mut lo, mut hi) = (0, 256 << 20);
        assert!(fits(levels, hi, mode), "{levels} levels at {hi}");
        while hi - lo > 4 << 10 {
            let mid = usize::midpoint(lo, hi) / 4096 * 4096;
            if fits(levels, mid, mode) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    };
    let mut lifted = Limits::default();
    lifted.max_container_depth = LIFTED_CONTAINER_DEPTH;
    let have = worker_stack(&lifted);
    for mode in ["eager", "reverse"] {
        let mut prev: Option<(u32, usize)> = None;
        for levels in [
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
    println!("worker_stack with the container depth lifted: {have}");
}
