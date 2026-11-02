//! The `unityrs dump` options that no oracle run replays (`index_runs.rs`
//! replays `--index`, `--kind`, `--build` and `--name`), each taking effect
//! through the binary, where a gate takes them: `--reverse-prepass` (G1c),
//! `-j` (G1b), `--unbounded` (G3b's re-run of a limit refusal) and `-q`.
//!
//! Review round 14: the tests of the first three stopped at the library
//! (`DumpOptions`), so `main.rs` could drop any of them with every test
//! passing, and G1b and G1c would then compare two runs that differ in
//! nothing. What the records cannot show, `--stats` does: each input's
//! `decode_work` (the work its meter counted) and the run's `workers` (the
//! size of the pool the inputs were dumped on, as their workers saw it).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value as Json;
use unityrs_dump::DumpOptions;
use unityrs_dump::input::dump_on;
use unityrs_dump::read::{ReadInput, read_input};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;
use unityrs_testkit::serialized::{self, one_object};

/// A folder outside the repository, its path resolved, holding the input
/// folder `in`; removed when dropped (`unityrs_testkit::scratch`).
struct Tmp(
    PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")] Scratch,
);

impl Tmp {
    fn new(what: &str, inputs: &[(&str, Vec<u8>)]) -> Self {
        let dir = Scratch::new(&format!("cli-flags-{what}"));
        let base = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(base.join("in")).unwrap();
        for (name, bytes) in inputs {
            std::fs::write(base.join("in").join(name), bytes).unwrap();
        }
        Self(base, dir)
    }

    fn input(&self) -> PathBuf {
        self.0.join("in")
    }

    /// `unityrs dump in -o <run>.jsonl --stats <run>.json` with `args`,
    /// which must exit 0: the dump's bytes, its stats, and its stderr.
    fn dump(&self, run: &str, args: &[&str]) -> (Vec<u8>, Json, String) {
        let (out, stats) = (
            self.0.join(format!("{run}.jsonl")),
            self.0.join(format!("{run}.json")),
        );
        let o = unityrs(
            Command::new(env!("CARGO_BIN_EXE_unityrs"))
                .arg("dump")
                .arg(self.input())
                .arg("-o")
                .arg(&out)
                .arg("--stats")
                .arg(&stats)
                .args(args),
        );
        let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
        assert_eq!(o.status.code(), Some(0), "{run} {args:?}: {stderr}");
        let stats = serde_json::from_slice(&std::fs::read(&stats).unwrap()).unwrap();
        (std::fs::read(&out).unwrap(), stats, stderr)
    }
}

/// A run of the binary; one the system kills from outside is run again
/// (`unityrs_testkit::child`).
fn unityrs(cmd: &mut Command) -> Output {
    child::output(cmd).expect("the unityrs binary runs")
}

fn records(dump: &[u8]) -> Vec<Json> {
    dump.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect()
}

/// A SerializedFile of one `int` object (path id 1).
fn cab() -> Vec<u8> {
    serialized::build(&one_object(
        1,
        vec![
            NodeSpec::new(0, "GameObject", "Base"),
            NodeSpec::new(1, "int", "m_X").size(4),
        ],
        vec![7, 0, 0, 0],
    ))
}

/// A bundle of blocks of 128 bytes whose entries span several each, a
/// nested bundle of such blocks among them (`unityrs-dump`'s
/// `tests/testkit.rs`, whose `the_reverse_prepass_parses_every_entry_lazily_before_the_load`
/// shows the pre-pass's work on it).
fn blocks_bundle() -> Vec<u8> {
    let small = |entries: Vec<(&[u8], Vec<u8>, u32)>| {
        build(&BundleSpec {
            block_size: 128,
            entries: entries
                .into_iter()
                .map(|(name, data, flags)| EntrySpec {
                    name: name.to_vec(),
                    data,
                    flags,
                })
                .collect(),
            ..BundleSpec::default()
        })
    };
    let inner = small(vec![(b"CAB-a", cab(), 4), (b"CAB-b", cab(), 4)]);
    small(vec![
        (b"CAB-1", cab(), 4),
        (b"inner.bundle", inner, 0),
        (b"CAB-2", cab(), 4),
        (b"CAB-2.resS", vec![9; 300], 0),
    ])
}

/// `levels` bundles each holding the next as its one entry, around
/// [`cab`]: the SerializedFile is at container depth `levels`, so the
/// default `max_container_depth` (16) reads 17 levels and refuses 18
/// (`unityrs-dump`'s `tests/limits.rs`, `nest`).
fn nest(levels: usize) -> Vec<u8> {
    let mut inner = cab();
    let mut flags = 4;
    for _ in 0..levels {
        inner = build(&BundleSpec {
            info_codec: TestCodec::None,
            data_codec: TestCodec::None,
            entries: vec![EntrySpec {
                name: b"n".to_vec(),
                data: inner,
                flags,
            }],
            ..BundleSpec::default()
        });
        flags = 0;
    }
    inner
}

/// The work the run's own steps count on `path`'s input, named `name`,
/// with the pre-pass or without: [`read_input`]'s read on a fresh meter
/// of the input's, then [`dump_on`] on it, as `run::write_dump` does.
fn library_work(name: &[u8], path: &Path, lazy_reverse: bool) -> u64 {
    let opts = DumpOptions {
        lazy_reverse,
        ..DumpOptions::default()
    };
    let ReadInput::Bytes { data, meter } = read_input(name, path, &opts.context).unwrap() else {
        panic!("{}: refused", path.display())
    };
    let d = dump_on(name, data, &meter, &opts);
    assert!(!d.text().contains("\"error\":{"), "{}", d.text());
    meter.decoded()
}

/// G1c through the binary: `--reverse-prepass` changes no byte of the dump
/// and adds exactly the pre-pass's work, the input's `decode_work` being
/// the work the library counts with `DumpOptions::lazy_reverse` (whose
/// test shows that is the pre-pass's), and without it the work it counts
/// without.
#[test]
fn reverse_prepass_adds_the_prepass_and_changes_no_record() {
    let tmp = Tmp::new("prepass", &[("p.bundle", blocks_bundle())]);
    let (eager, eager_stats, _) = tmp.dump("eager", &["-q"]);
    let (lazy, lazy_stats, _) = tmp.dump("lazy", &["-q", "--reverse-prepass"]);
    assert!(eager == lazy, "the pre-pass changed the dump");
    assert!(records(&eager).len() > 8, "{}", records(&eager).len());
    let work = |stats: &Json| {
        let per = stats["inputs"][0]["decode_work"].as_u64().unwrap();
        assert_eq!(stats["summary"]["decode_work"].as_u64(), Some(per));
        per
    };
    let path = tmp.input().join("p.bundle");
    let (want_eager, want_lazy) = (
        library_work(b"p.bundle", &path, false),
        library_work(b"p.bundle", &path, true),
    );
    assert!(want_eager < want_lazy, "{want_eager} {want_lazy}");
    assert_eq!(work(&eager_stats), want_eager, "without --reverse-prepass");
    assert_eq!(work(&lazy_stats), want_lazy, "with --reverse-prepass");
}

/// G1b through the binary: `-j N` dumps the inputs on a pool of N workers,
/// or of one per input when there are fewer (`run::ordered_on`, whose
/// tests show a pool of N dumps N inputs at once), and the dump is the
/// same at every `-j`.
#[test]
fn jobs_sets_how_many_workers_dump_the_inputs() {
    let tmp = Tmp::new(
        "jobs",
        &[
            ("0.assets", cab()),
            ("1.assets", cab()),
            ("2.assets", cab()),
            ("b.bundle", blocks_bundle()),
        ],
    );
    let (one, stats, _) = tmp.dump("j1", &["-q"]);
    assert_eq!(stats["summary"]["workers"], 1, "no -j");
    for (jobs, workers) in [("1", 1), ("2", 2), ("3", 3), ("8", 4)] {
        let (dump, stats, _) = tmp.dump(&format!("j{jobs}"), &["-q", "-j", jobs]);
        assert_eq!(stats["summary"]["workers"], workers, "-j {jobs}");
        assert!(dump == one, "-j {jobs} differs from no -j");
    }
}

/// G3b's re-run through the binary: `--unbounded ContainerDepth` dumps
/// whole the nest the default bound refuses, and the header names what it
/// lifted, so no gate takes such a dump for its own.
#[test]
fn unbounded_lifts_the_bound_it_names() {
    let tmp = Tmp::new("unbounded", &[("nest", nest(18))]);
    let (bounded, _, _) = tmp.dump("bounded", &["-q"]);
    let recs = records(&bounded);
    assert_eq!(recs.len(), 3, "the header, the input's refusal, the end");
    assert_eq!(
        recs[1]["error"],
        serde_json::json!({"kind": "parse", "note": "limit ContainerDepth: 17 > 16"})
    );
    assert!(
        recs[0]["producer"].get("unbounded").is_none(),
        "{}",
        recs[0]
    );

    let (lifted, _, _) = tmp.dump("lifted", &["-q", "--unbounded", "ContainerDepth"]);
    let recs = records(&lifted);
    let files = recs.iter().filter(|r| r["rec"] == "file").count();
    assert_eq!(files, 19, "18 bundles and the SerializedFile");
    assert!(
        recs.iter()
            .all(|r| r.get("error").is_none_or(Json::is_null)),
        "{recs:?}"
    );
    assert_eq!(
        recs[0]["producer"]["unbounded"],
        serde_json::json!(["ContainerDepth"])
    );
}

/// `-q`: no per-input progress line on stderr; without it, one per input.
#[test]
fn quiet_hides_the_progress() {
    let tmp = Tmp::new("quiet", &[("a", b"a".to_vec()), ("b", b"b".to_vec())]);
    let progress = |stderr: &str| -> Vec<String> {
        stderr
            .lines()
            .filter(|l| l.starts_with('['))
            .map(|l| l.split(':').next().unwrap_or_default().to_owned())
            .collect()
    };
    let (_, _, loud) = tmp.dump("loud", &[]);
    assert_eq!(progress(&loud), ["[1/2] a", "[2/2] b"], "{loud}");
    let (_, _, quiet) = tmp.dump("quiet", &["-q"]);
    assert_eq!(progress(&quiet), Vec::<String>::new(), "{quiet}");
    assert!(
        quiet.contains("\"inputs\": 2"),
        "the summary stays: {quiet}"
    );
}
