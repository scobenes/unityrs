//! The record writer and the dump driver on hand-built records (plan 7.6,
//! wave 1: the core's files are not built yet, so the records stand in for
//! them): every record type of `docs/CANONICAL_DUMP.md` section 4 and every
//! value of `tests/fixtures/dump/encode.json` through `tools/parity/compare.py`'s
//! canonical checks, at each level set; the output's independence of `-j`;
//! gzip with mtime 0; the `.partial` file and its rename; the stops.

#[path = "../common/mod.rs"]
mod common;

mod hand_built;

use common::{Tmp, compare, gunzip, line_problems, repo, s};
use hand_built::{
    corpus, error, file, l0, l1_of, l3_of, raw, records, serialized_l1, stream, values,
};
use unityrs_core::Bytes;
use unityrs_core::detect::{FileType, ParsedAs};
use unityrs_core::walk::L2Kind;
use unityrs_dump::json::sha256;
use unityrs_dump::record::{FileL1, ObjectRecord, object_line};
use unityrs_dump::run::{RunError, RunOptions, run_with};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, InputDump, Levels};

const LEVEL_SETS: [Levels; 4] = [
    Levels {
        l1: false,
        l2: false,
        l3: false,
    },
    Levels {
        l1: true,
        l2: false,
        l3: false,
    },
    Levels {
        l1: true,
        l2: true,
        l3: false,
    },
    Levels::ALL,
];

#[test]
fn every_record_type_passes_compare_py_at_every_level_set() {
    let (tmp, inputs) = corpus(3);
    for levels in LEVEL_SETS {
        let text = stream(&inputs, levels, 1);
        let path = tmp
            .0
            .join(format!("dump-{}.jsonl", levels.names().join("")));
        std::fs::write(&path, &text).unwrap();
        let (status, report) = compare(&path, &path);
        assert_eq!(status, 0, "{levels:?}:\n{report}");
        assert_eq!(line_problems(&path), "", "{levels:?}");
        // The header and the end record.
        let lines: Vec<&[u8]> = text.split(|b| *b == b'\n').collect();
        assert!(lines[0].starts_with(b"{\"rec\":\"dump\""));
        assert_eq!(
            lines.last(),
            Some(&&b""[..]),
            "the stream ends with a newline"
        );
        let end = lines[lines.len() - 2];
        let files = lines
            .iter()
            .filter(|l| l.starts_with(b"{\"rec\":\"file\""))
            .count();
        let objects = lines
            .iter()
            .filter(|l| l.starts_with(b"{\"rec\":\"object\""))
            .count();
        assert_eq!(
            String::from_utf8_lossy(end),
            format!("{{\"rec\":\"end\",\"files\":{files},\"objects\":{objects}}}")
        );
        assert!(objects >= 70, "{objects} objects");
    }
}

#[test]
fn a_record_compare_py_refuses_is_caught() {
    // The check above must be able to fail: a dump whose end record
    // miscounts, and one with a summary of 64 bytes, are each refused.
    let (tmp, inputs) = corpus(0);
    let text = String::from_utf8(stream(&inputs, Levels::ALL, 1)).unwrap();
    let bad_end = text.replace("\"rec\":\"end\",\"files\":", "\"rec\":\"end\",\"files\":1");
    let path = tmp.0.join("bad-end.jsonl");
    std::fs::write(&path, bad_end).unwrap();
    let (status, report) = compare(&path, &path);
    assert_eq!(status, 1, "{report}");
    let short = format!(
        "{{\"t\":\"bytes\",\"len\":64,\"sha256\":\"{}\"}}",
        "0".repeat(64)
    );
    let bad_value = text.replacen("{\"t\":\"dict\",\"v\":[]}", &short, 1);
    let path = tmp.0.join("bad-value.jsonl");
    std::fs::write(&path, bad_value).unwrap();
    assert!(line_problems(&path).contains("summary_too_short"));
}

#[test]
fn an_encode_error_is_canonical_in_a_candidate() {
    let tmp = Tmp::new("encode");
    let me = [raw(b"x")];
    let rec = ObjectRecord {
        path_id: 1,
        l1: Some(l1_of(1)),
        l2: Some(error(L2Kind::Encode, None, None)),
        l3: Some(l3_of(0, false)),
    };
    let mut d = InputDump::default();
    file(
        &mut d,
        &me,
        l0(Some(FileType::AssetsFile), ParsedAs::SerializedFile, b"x"),
        Some(FileL1::Serialized(Box::new(serialized_l1(vec![1], None)))),
        Levels::ALL,
    );
    d.lines.push(object_line(&me, &rec));
    let text = format!(
        "{}\n{}{}\n",
        unityrs_dump::header_line(Levels::ALL, "unityrs"),
        d.text(),
        unityrs_dump::end_line(1, 1)
    );
    let path = tmp.0.join("encode.jsonl");
    std::fs::write(&path, text).unwrap();
    assert_eq!(line_problems(&path), "");
}

#[test]
fn the_output_does_not_depend_on_jobs() {
    let (_tmp, inputs) = corpus(60);
    let one = stream(&inputs, Levels::ALL, 1);
    for jobs in [2, 3, 8] {
        assert!(
            stream(&inputs, Levels::ALL, jobs) == one,
            "-j {jobs} differs from -j 1"
        );
    }
}

fn options(tmp: &Tmp, output: &str, jobs: usize) -> RunOptions {
    RunOptions {
        selection: Selection {
            inputs: vec![tmp.0.join("in")],
            ..Selection::default()
        },
        output: tmp.0.join("out").join(output),
        dump: DumpOptions {
            jobs,
            ..DumpOptions::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    }
}

#[test]
fn a_run_writes_gzip_with_mtime_0_through_a_partial_file_at_any_jobs() {
    let (tmp, inputs) = corpus(20);
    let vals = values();
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &vals);
    let want = stream(&inputs, Levels::ALL, 1);
    for jobs in [1, 4] {
        let opts = options(&tmp, &format!("j{jobs}.jsonl.gz"), jobs);
        // A .partial left by a killed run is removed first.
        let partial = tmp.0.join("out").join(format!("j{jobs}.jsonl.gz.partial"));
        std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
        std::fs::write(&partial, b"stale").unwrap();
        let summary = run_with(&opts, &dumper).unwrap();
        assert!(!partial.exists(), "the .partial file was renamed");
        let gz = std::fs::read(&opts.output).unwrap();
        assert_eq!(&gz[..10], b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff");
        assert!(gunzip(&gz) == want, "-j {jobs}");
        assert_eq!(summary.inputs, inputs.len() as u64);
    }
    // Without .gz the stream is plain.
    let opts = options(&tmp, "plain.jsonl", 2);
    run_with(&opts, &dumper).unwrap();
    assert_eq!(sha256(&std::fs::read(&opts.output).unwrap()), sha256(&want));
}

#[test]
fn a_panic_stops_the_run_and_writes_no_dump() {
    let (tmp, _) = corpus(10);
    for jobs in [1, 3] {
        let opts = options(&tmp, "panic.jsonl", jobs);
        let dumper = |name: &[u8], _: Bytes| {
            assert!(name != b"e/plain-004", "a bug");
            InputDump::default()
        };
        let err = run_with(&opts, &dumper).unwrap_err();
        assert!(matches!(err, RunError::Failed(_)), "{err:?}");
        assert!(err.to_string().contains("a bug"), "{err}");
        assert!(!opts.output.exists());
        assert!(!tmp.0.join("out/panic.jsonl.partial").exists());
    }
}

#[test]
fn an_index_mismatch_stops_the_run_unless_allowed() {
    let tmp = Tmp::new("index");
    std::fs::create_dir_all(tmp.0.join("corpus")).unwrap();
    std::fs::write(tmp.0.join("corpus/one.bin"), b"one").unwrap();
    std::fs::write(tmp.0.join("corpus/two.bin"), b"two").unwrap();
    let good = unityrs_dump::json::sha256_hex(b"one");
    let index = serde_json::json!({"files": [
        {"path": "one.bin", "kind": "bundle", "sha256": good},
        {"path": "two.bin", "kind": "bundle", "sha256": "00".repeat(32)},
    ]});
    std::fs::write(tmp.0.join("corpus/index.json"), index.to_string()).unwrap();
    let mut opts = options(&tmp, "index.jsonl", 2);
    opts.selection = Selection {
        inputs: vec![tmp.0.join("corpus/index.json")],
        index: true,
        ..Selection::default()
    };
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &[]);
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("two.bin: sha256"), "{err}");
    assert_eq!(err.status(), 1);
    assert!(!opts.output.exists());
    opts.allow_index_mismatch = true;
    let summary = run_with(&opts, &dumper).unwrap();
    assert_eq!(summary.index_sha256_mismatch, 1);
    assert_eq!(summary.files, 2);
}

#[test]
fn paths_the_oracle_refuses_are_refused() {
    let (tmp, _) = corpus(1);
    let dumper = |_: &[u8], _: Bytes| InputDump::default();
    // Inside the repository.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.output = repo().join("target/never-written.jsonl");
    let err = run_with(&opts, &dumper).unwrap_err();
    assert_eq!(err.status(), 2, "{err}");
    assert!(!opts.output.exists());
    // A symlink at the output.
    let opts = options(&tmp, "link.jsonl", 1);
    std::fs::create_dir_all(opts.output.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(tmp.0.join("elsewhere"), &opts.output).unwrap();
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
    // An input as the output.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.output = tmp.0.join("in/e/plain-000");
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("would overwrite"), "{err}");
    assert_eq!(std::fs::read(&opts.output).unwrap(), b"e/plain-000");
    // The stats over the dump.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.stats = Some(opts.output.clone());
    assert_eq!(run_with(&opts, &dumper).unwrap_err().status(), 2);
    // No input at all.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.selection.names = vec![b"nothing*".to_vec()];
    let err = run_with(&opts, &dumper).unwrap_err();
    assert_eq!(err, RunError::Usage("no inputs matched".into()));
}

#[test]
fn stats_are_written_beside_the_dump() {
    let (tmp, _) = corpus(2);
    let mut opts = options(&tmp, "s.jsonl", 2);
    opts.stats = Some(tmp.0.join("out/stats.json"));
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &[]);
    run_with(&opts, &dumper).unwrap();
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.0.join("out/stats.json")).unwrap()).unwrap();
    assert_eq!(stats["summary"]["inputs"], 9);
    assert_eq!(stats["inputs"].as_array().unwrap().len(), 9);
    assert_eq!(s(&stats["inputs"][0]["path"]), "a/crash.bin");
}
