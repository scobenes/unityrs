//! The dump on testkit-built inputs, read through the core (plan 7.6's
//! tests): the format matrix (`unityrs_testkit::bundle::matrix`, G3's
//! inputs) dumps in path order and the same at any `-j`, with no format
//! problem `tools/parity/compare.py` can find at any level set; and a
//! failing entry fails its input (section 5: UnityPy's eager load parses
//! every entry, nested bundles' included, and raises on the first failure).
//!
//! With `UNITYRS_WRITE_DUMP_CASES=DIR` set, `failing_entries` writes its
//! inputs into DIR instead of testing them, so the oracle can dump the same
//! bytes (`python tools/parity/unitypy_dump.py DIR -o ...`); its records are
//! what these tests assert.

mod common;

use common::{Tmp, compare, line_problems};
use serde_json::Value as Json;
use unityrs_core::Bytes;
use unityrs_dump::run::{RunOptions, run};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, Levels};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, build, matrix};
use unityrs_testkit::serialized::{self, one_object};

const LEVEL_SETS: [&str; 4] = ["L0", "L0,L1", "L0,L1,L2", "L0,L1,L2,L3"];

/// The matrix, written under `tmp/in`.
fn matrix_folder(tmp: &Tmp) -> Vec<(String, Vec<u8>)> {
    let cases = matrix();
    let dir = tmp.0.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in &cases {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    cases
}

/// `unityrs dump tmp/in -o tmp/out/NAME` (plain JSON Lines), its bytes.
fn dump(tmp: &Tmp, name: &str, levels: Levels, jobs: usize) -> Vec<u8> {
    let opts = RunOptions {
        selection: Selection {
            inputs: vec![tmp.0.join("in")],
            ..Selection::default()
        },
        output: tmp.0.join("out").join(name),
        dump: DumpOptions {
            levels,
            jobs,
            ..DumpOptions::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    run(&opts).unwrap();
    std::fs::read(&opts.output).unwrap()
}

fn records(text: &[u8]) -> Vec<Json> {
    text.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect()
}

#[test]
fn the_matrix_dumps_in_path_order_and_the_same_at_any_jobs() {
    let tmp = Tmp::new("testkit-order");
    let cases = matrix_folder(&tmp);
    let one = dump(&tmp, "j1.jsonl", Levels::ALL, 1);
    for jobs in [2, 8] {
        assert!(
            dump(&tmp, &format!("j{jobs}.jsonl"), Levels::ALL, jobs) == one,
            "-j {jobs} differs from -j 1"
        );
    }
    let recs = records(&one);
    // Each input's own record comes first, the inputs in bytewise order of
    // their names, and each input's records are contiguous.
    let mut names: Vec<&str> = cases.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    let tops: Vec<&str> = recs
        .iter()
        .filter(|r| r["rec"] == "file" && r["file"].as_array().unwrap().len() == 1)
        .map(|r| r["file"][0].as_str().unwrap())
        .collect();
    assert_eq!(tops, names);
    let firsts: Vec<&str> = recs
        .iter()
        .filter(|r| r["rec"] != "dump" && r["rec"] != "end")
        .map(|r| r["file"][0].as_str().unwrap())
        .collect();
    let mut runs = firsts.clone();
    runs.dedup();
    assert_eq!(runs, names, "each input's records are contiguous");
    // Inside a SerializedFile, objects by signed path id.
    let mut last: Option<(Json, i64)> = None;
    for r in &recs {
        if r["rec"] == "object" {
            let id = r["path_id"].as_i64().unwrap();
            if let Some((f, prev)) = &last
                && *f == r["file"]
            {
                assert!(*prev <= id, "{f}: {prev} before {id}");
            }
            last = Some((r["file"].clone(), id));
        }
    }
    let end = recs.last().unwrap();
    assert_eq!(end["rec"], "end");
    let count = |what: &str| recs.iter().filter(|r| r["rec"] == what).count() as u64;
    assert_eq!(end["files"], count("file"));
    assert_eq!(end["objects"], count("object"));
    assert!(count("object") >= 200, "{} objects", count("object"));
}

#[test]
fn compare_py_finds_no_format_problem_in_a_testkit_dump() {
    let tmp = Tmp::new("testkit-compare");
    matrix_folder(&tmp);
    for list in LEVEL_SETS {
        let levels = Levels::parse(list).unwrap();
        let name = format!("{}.jsonl", list.replace(',', ""));
        dump(&tmp, &name, levels, 2);
        let path = tmp.0.join("out").join(&name);
        let (status, report) = compare(&path, &path);
        assert_eq!(status, 0, "{list}:\n{report}");
        assert_eq!(line_problems(&path), "", "{list}");
    }
}

/// A SerializedFile of one `int` object (path id 1).
fn good_cab() -> Vec<u8> {
    serialized::build(&one_object(
        1,
        vec![
            NodeSpec::new(0, "GameObject", "Base"),
            NodeSpec::new(1, "int", "m_X").size(4),
        ],
        vec![7, 0, 0, 0],
    ))
}

fn bundle(entries: Vec<(&[u8], Vec<u8>, u32)>) -> Vec<u8> {
    build(&BundleSpec {
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
}

/// Bundles with one entry that fails, and one without, each `(name, bytes,
/// fails)`.
fn failing_entries() -> Vec<(&'static str, Vec<u8>, bool)> {
    let from_matrix = |name: &str| {
        matrix()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b)
            .unwrap()
    };
    // The AssetBundle read at load fails `check_read` (4 of 8 bytes read),
    // which fails its file (`SerializedFile.py:316-324`), so its bundle.
    let bad_asset_bundle = serialized::build(&one_object(
        142,
        vec![
            NodeSpec::new(0, "AssetBundle", "Base"),
            NodeSpec::new(1, "int", "m_X").size(4),
        ],
        vec![1, 0, 0, 0, 2, 0, 0, 0],
    ));
    let bad_tree = from_matrix("CAB-negative-node-count");
    let fails_parse = bundle(vec![
        (b"CAB-good", good_cab(), 4),
        (b"CAB-bad", bad_tree.clone(), 4),
    ]);
    vec![
        (
            "whole.bundle",
            bundle(vec![
                (b"CAB-good", good_cab(), 4),
                (b"CAB-good.resS", vec![1, 2, 3], 0),
            ]),
            false,
        ),
        ("entry-fails-parse.bundle", fails_parse.clone(), true),
        (
            "first-entry-fails.bundle",
            bundle(vec![
                (b"CAB-bad", bad_tree, 4),
                (b"CAB-good", good_cab(), 4),
            ]),
            true,
        ),
        // The failing entry is shadowed by a later one of the same path, so
        // `files` never holds it: only the eager parse of every directory
        // entry (`File.py:76-90`) meets it.
        (
            "shadowed-entry-fails.bundle",
            bundle(vec![
                (b"CAB-x", from_matrix("CAB-negative-node-count"), 4),
                (b"CAB-x", good_cab(), 4),
            ]),
            true,
        ),
        (
            "entry-fails-asset-bundle.bundle",
            bundle(vec![
                (b"CAB-good", good_cab(), 4),
                (b"CAB-ab", bad_asset_bundle, 4),
            ]),
            true,
        ),
        (
            "nested-entry-fails.bundle",
            bundle(vec![
                (b"CAB-good", good_cab(), 4),
                (b"inner.bundle", fails_parse, 0),
            ]),
            true,
        ),
    ]
}

#[test]
fn a_failing_entry_fails_its_input() {
    let cases = failing_entries();
    if let Some(dir) = std::env::var_os("UNITYRS_WRITE_DUMP_CASES") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes, _) in &cases {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        return;
    }
    let opts = DumpOptions::default();
    for (name, bytes, fails) in cases {
        let d = unityrs_dump::dump_input(name.as_bytes(), Bytes::from(bytes), &opts);
        let recs = records(d.text().as_bytes());
        if fails {
            // One record: the input, parsed as an error of kind parse.
            assert_eq!(recs.len(), 1, "{name}: {recs:?}");
            let r = &recs[0];
            assert_eq!(r["file"], serde_json::json!([name]), "{name}");
            assert_eq!(r["l0"]["type"], "BundleFile", "{name}");
            assert_eq!(r["l0"]["parsed_as"], "error", "{name}");
            assert_eq!(r["error"]["kind"], "parse", "{name}");
            assert_eq!((d.files, d.objects), (1, 0), "{name}");
        } else {
            // The bundle, its CAB and its object, and the resource.
            let kinds: Vec<(&str, usize)> = recs
                .iter()
                .map(|r| {
                    (
                        r["rec"].as_str().unwrap(),
                        r["file"].as_array().unwrap().len(),
                    )
                })
                .collect();
            assert_eq!(
                kinds,
                [("file", 1), ("file", 2), ("object", 2), ("file", 2)],
                "{name}"
            );
            assert!(
                recs.iter()
                    .all(|r| r.get("error").is_none_or(Json::is_null))
            );
            assert_eq!(recs[2]["l2"]["error"], Json::Null, "{name}");
        }
    }
}
