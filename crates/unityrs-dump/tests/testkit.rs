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

use std::sync::Arc;

use common::{Tmp, compare, line_problems};
use serde_json::Value as Json;
use unityrs_core::budget::Reservation;
use unityrs_core::detect::check_entry_type;
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_dump::input::{dump_on, load_deep, prepass_reverse};
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

/// G1c on testkit inputs (plan section 6): parsing every bundle's entries
/// lazily in reverse directory order before the dump's own pass changes no
/// record, on the format matrix (nested bundles, repeated paths, block
/// overrides) and on the failing-entry bundles.
#[test]
fn the_lazy_reverse_prepass_changes_nothing() {
    let lazy = DumpOptions {
        lazy_reverse: true,
        ..DumpOptions::default()
    };
    let eager = DumpOptions::default();
    let inputs = matrix().into_iter().chain(
        failing_entries()
            .into_iter()
            .map(|(n, b, _)| (n.to_owned(), b)),
    );
    let mut bundles = 0;
    for (name, bytes) in inputs {
        let data = Bytes::from(bytes);
        let a = unityrs_dump::dump_input(name.as_bytes(), data.clone(), &eager);
        let b = unityrs_dump::dump_input(name.as_bytes(), data, &lazy);
        assert_eq!(a, b, "{name}");
        bundles += usize::from(a.text().contains("\"BundleFile\""));
    }
    assert!(bundles >= 20, "{bundles} bundles");
}

/// A bundle of blocks of 128 bytes whose entries span several each, a
/// nested bundle of such blocks among them.
fn prepass_bundle() -> Vec<u8> {
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
    let inner = small(vec![(b"CAB-a", good_cab(), 4), (b"CAB-b", good_cab(), 4)]);
    small(vec![
        (b"CAB-1", good_cab(), 4),
        (b"inner.bundle", inner, 0),
        (b"CAB-2", good_cab(), 4),
        (b"CAB-2.resS", vec![9; 300], 0),
    ])
}

/// Every entry of `file` at every depth, read (`BundleFile::entry`) in
/// directory order, or in reverse directory order with `reverse`, a nested
/// bundle's entries as soon as it is read.
fn read_every_entry(file: &File, reverse: bool) {
    if let File::Bundle(b) = file {
        let n = b.directory().len();
        for k in 0..n {
            let i = if reverse { n - 1 - k } else { k };
            read_every_entry(&b.entry(i).unwrap(), reverse);
        }
    }
}

/// Review round 13: every test of the pre-pass compared the dump with it
/// to the dump without it, so a pre-pass that did nothing passed them all,
/// and G1c with it. The work an input's meter counts shows what it does on
/// a bundle of many blocks with a nested bundle: exactly the work of
/// reading every entry lazily (`BundleFile::entry`, each read decoding the
/// blocks its span needs) in reverse directory order, a nested bundle's
/// entries as soon as it is read, which differs from the work in directory
/// order; every entry at every depth parsed and kept (reading them all
/// again counts nothing); no bundle's joined data read whole (the eager
/// load after it still decodes that, and parses no entry); and `dump_on`
/// with [`DumpOptions::lazy_reverse`] does exactly that much more work
/// than without it, so it runs the pre-pass before its load.
#[test]
fn the_reverse_prepass_parses_every_entry_lazily_before_the_load() {
    let opts = DumpOptions::default();
    let ctx = &opts.context;
    let data = Bytes::from(prepass_bundle());
    let meter = || Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let open = |meter: &Arc<Meter>| {
        let ty = check_entry_type(&data, &ctx.limits, meter).unwrap();
        parse_file(
            RawStr::from(&b"p.bundle"[..]),
            data.clone(),
            ty,
            None,
            meter,
            ctx,
        )
        .unwrap()
    };
    // The work of `then` on a fresh open of the bundle, after its open.
    let fresh = |then: &dyn Fn(&File)| {
        let m = meter();
        let file = open(&m);
        let opened = m.decoded();
        then(&file);
        m.decoded() - opened
    };
    let (forward, backward) = (
        fresh(&|f| read_every_entry(f, false)),
        fresh(&|f| read_every_entry(f, true)),
    );
    assert_ne!(forward, backward, "the bundle tells the two orders apart");
    let load = fresh(&|f| load_deep(f).unwrap());

    let m = meter();
    let file = open(&m);
    let opened = m.decoded();
    prepass_reverse(&file);
    let prepass = m.decoded() - opened;
    assert_eq!(
        prepass, backward,
        "the pre-pass is not every entry read lazily in reverse directory order \
         (in directory order: {forward})"
    );
    read_every_entry(&file, false);
    assert_eq!(
        m.decoded() - opened,
        prepass,
        "an entry the pre-pass did not parse and keep"
    );
    load_deep(&file).unwrap();
    let load_after = m.decoded() - opened - prepass;
    assert!(
        0 < load_after && load_after < load,
        "after the pre-pass the eager load decodes the joined data ({load_after}) and parses \
         no entry (fresh: {load})"
    );

    let lazy = DumpOptions {
        lazy_reverse: true,
        ..DumpOptions::default()
    };
    let work = |opts: &DumpOptions| {
        let m = meter();
        let d = dump_on(b"p.bundle", data.clone(), &m, opts);
        assert!(!d.text().contains("\"error\":{"), "{}", d.text());
        m.decoded()
    };
    let (with, without) = (work(&lazy), work(&opts));
    assert_eq!(
        with + load,
        without + prepass + load_after,
        "dump_on with lazy_reverse does the pre-pass's work before its load, and no more"
    );
}

/// `CANONICAL_DUMP` section 7's crash divergence, unityrs's side: UnityPyBoost
/// dies loading a SerializedFile whose node name (`SIGABRT`) or node type
/// (`SIGSEGV`) is not UTF-8, so the oracle writes its crash record; unityrs
/// reads the file and dumps it whole, the bytes hex-tagged (`PARITY.md`
/// section 19.3). A name the oracle would crash on is a field name; a type
/// it would crash on reads as a class of no known kind.
#[test]
fn a_node_name_or_type_that_is_not_utf8_dumps_whole_hex_tagged() {
    for (field, ty) in [(&b"m_\xffX"[..], &b"int"[..]), (b"m_X", b"in\xfft")] {
        let mut nodes = vec![
            NodeSpec::new(0, "GameObject", "Base"),
            NodeSpec::new(1, "int", "m_X").size(4),
        ];
        nodes[1].name = field.to_vec();
        nodes[1].ty = ty.to_vec();
        let data = serialized::build(&one_object(1, nodes, vec![7, 0, 0, 0]));
        let d =
            unityrs_dump::dump_input(b"CAB-bad-name", Bytes::from(data), &DumpOptions::default());
        let recs = records(d.text().as_bytes());
        assert_eq!(recs.len(), 2, "{recs:?}");
        assert!(recs[0]["error"].is_null(), "{:?}", recs[0]);
        let bad = if field == b"m_X" { ty } else { field };
        let tagged = serde_json::json!({"t": "str", "hex": hex(bad)});
        let row = &recs[0]["l1"]["types"][0]["nodes"][1];
        assert!(row.as_array().unwrap().contains(&tagged), "{row}");
        assert!(
            recs[1]["l2"]["error"].is_null() || ty != b"int",
            "{:?}",
            recs[1]["l2"]
        );
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}
