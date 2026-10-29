//! G7 (`docs/plans/core.md` section 6): for every class a TPK holds, at every
//! version it lists, [`TpkProvider`] gives UnityPy 1.25.2's outcome: the node
//! list `helpers/Tpk.py` builds, "no node" (`Ok(None)`) where it raises
//! `KeyError`, `ValueError` or `AssertionError`, and an error where it raises
//! anything else; and the TPK's class count is UnityPy's.
//!
//! Each answer is checked twice: the node list ([`TpkProvider::node_list`])
//! and the compiled tree ([`TypeTreeProvider::tree`]), which must be the
//! node list's, compiled, wherever UnityPy builds one.
//!
//! Two sets. The synthetic TPKs of `tools/fixtures/tpk.py`, committed with
//! UnityPy's outcomes under `tests/fixtures/tpk/`, are replayed here without
//! Python; a case marked `refused` is one UnityPy loads and unityrs refuses
//! by design (`PARITY.md` section 19.3), and must be refused for the reason
//! it names. UnityPy's own TPK (`resources/lzma.tpk` in the oracle image) is
//! read at test time, never committed: `tools/fixtures/tpk.py --check` writes
//! its outcomes outside the repository and runs [`image_tpk`] with
//! `UNITYRS_ORACLE_TPK` and `UNITYRS_ORACLE_TPK_OUTCOMES` set; run by plain
//! `cargo test` in the image, the test asks the image's Python for both.
//! Outside the image it is skipped, and with `UNITYRS_ORACLE_TPK` set it never
//! is: a missing or empty TPK or outcome file fails it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::limits::Limits;
use unityrs_core::provider::{TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::{NodeList, NodeRow};
use unityrs_core::{LimitError, LimitKind, UnityVersion};
use unityrs_tpk::{TpkError, TpkProvider};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn synthetic() -> Vec<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tpk/synthetic.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fixture: J = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["unitypy"], "1.25.2");
    fixture["cases"].as_array().unwrap().clone()
}

fn case(name: &str) -> J {
    synthetic()
        .into_iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn limits(depth: u32) -> Limits {
    let mut limits = Limits::default();
    limits.max_tree_depth = depth;
    limits
}

fn open(case: &J) -> Result<TpkProvider, unityrs_tpk::TpkError> {
    let depth = u32::try_from(case["depth"].as_u64().unwrap()).unwrap();
    TpkProvider::from_bytes(&hex(case["tpk"].as_str().unwrap()), &limits(depth))
}

fn version(v: &J) -> UnityVersion {
    UnityVersion::from_packed(v.as_str().unwrap().parse().unwrap())
}

/// One node as UnityPy's `TypeTreeNode` holds it, in `tpk.py`'s order: level,
/// type, name, byte size, version, type flags, variable count, index, meta
/// flag, ref-type hash.
fn row_json(row: &NodeRow) -> J {
    let text = |s: &[u8]| J::from(std::str::from_utf8(s).unwrap());
    serde_json::json!([
        row.level,
        text(row.ty.as_bytes()),
        text(row.name.as_bytes()),
        row.byte_size,
        row.version,
        row.type_flags,
        row.variable_count,
        row.index,
        row.meta_flag,
        row.ref_type_hash,
    ])
}

/// Compares the provider's answers for one TPK's classes with UnityPy's
/// records (a header with the class count and versions, then a record per
/// class), and returns how many (class, version) pairs it checked.
fn compare<'r>(provider: &TpkProvider, head: &J, classes: impl Iterator<Item = &'r J>) -> usize {
    assert_eq!(head["load"], J::Null, "UnityPy loaded this TPK");
    let class_count = usize::try_from(head["class_count"].as_u64().unwrap()).unwrap();
    assert_eq!(provider.class_count(), class_count, "class count");
    let versions: Vec<UnityVersion> = head["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(version)
        .collect();
    assert_eq!(provider.versions(), &versions[..], "version list");
    let mut listed = 0;
    let mut pairs = 0;
    // A node list compared once per outcome it is checked against.
    let mut checked: HashSet<(usize, usize)> = HashSet::new();
    for record in classes {
        let class_id = i32::try_from(record["class"].as_i64().unwrap()).unwrap();
        listed += usize::from(provider.class_ids().contains(&class_id));
        let outcomes = record["outcomes"].as_array().unwrap();
        let picks = record["picks"].as_array().unwrap();
        assert_eq!(picks.len(), versions.len(), "class {class_id}");
        for (v, pick) in versions.iter().zip(picks) {
            let index = usize::try_from(pick.as_u64().unwrap()).unwrap();
            let want = &outcomes[index];
            let got = provider.node_list(class_id, v);
            let tree = provider.tree(&TreeRequest {
                class_id,
                version: v,
                script: None,
            });
            let at = || format!("class {class_id} at {:?}", v.packed());
            match (want.get("rows"), got, tree) {
                (Some(rows), Ok(Some(list)), Ok(Some(tree))) => {
                    assert!(Arc::ptr_eq(tree.list(), &list), "{}: the tree's list", at());
                    let key = (Arc::as_ptr(&list) as usize, index);
                    if checked.insert(key) {
                        compare_rows(&list, rows, &at());
                    }
                }
                (None, Ok(None), Ok(None)) => {
                    assert!(want.get("none").is_some(), "{}: {want}", at());
                }
                (None, Err(_), Err(_)) => assert!(want.get("error").is_some(), "{}: {want}", at()),
                (_, got, tree) => {
                    panic!("{}: UnityPy {want}, unityrs {got:?}, tree {tree:?}", at())
                }
            }
            pairs += 1;
        }
    }
    assert_eq!(
        listed, class_count,
        "every class the TPK holds has a record"
    );
    pairs
}

fn compare_rows(list: &Arc<NodeList>, want: &J, at: &str) {
    let want = want.as_array().unwrap();
    assert_eq!(list.rows().len(), want.len(), "{at}: row count");
    for (i, (row, want)) in list.rows().iter().zip(want).enumerate() {
        assert_eq!(&row_json(row), want, "{at}: row {i}");
    }
}

/// What a refusal names: a bound's `what`, or the unsupported codec.
fn refusal(e: &TpkError) -> String {
    match e {
        TpkError::TooLarge { what, .. } => (*what).to_owned(),
        TpkError::Unsupported(codec) => (*codec).to_owned(),
        e => format!("{e}"),
    }
}

#[test]
fn g7_the_synthetic_tpks_give_unitypys_outcomes() {
    let cases = synthetic();
    assert!(cases.len() >= 20, "{} cases", cases.len());
    let mut pairs = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let provider = open(case);
        if let Some(what) = case["refused"].as_str() {
            assert_eq!(case["load"], J::Null, "{name}: UnityPy refused it too");
            let e = provider.err().unwrap_or_else(|| panic!("{name}: loaded"));
            assert_eq!(refusal(&e), what, "{name}: {e}");
            continue;
        }
        if case["load"].is_string() {
            assert!(
                provider.is_err(),
                "{name}: UnityPy raised {}, unityrs loaded it",
                case["load"]
            );
            continue;
        }
        let provider =
            provider.unwrap_or_else(|e| panic!("{name}: UnityPy loaded it, unityrs: {e}"));
        pairs += compare(&provider, case, case["classes"].as_array().unwrap().iter());
    }
    assert!(pairs > 100, "{pairs} pairs");
}

/// The cases G7's synthetic set exists for are in it: each root flag
/// combination and the class after it, the type bytes rabex refused, the
/// 300-deep chain, both codecs and the streams on which liblz4 and liblzma
/// decide differently from rabex's decoders, and a case past each bound.
#[test]
fn g7_the_synthetic_set_covers_f6_f7_and_the_deep_chain() {
    let cases = synthetic();
    let names: Vec<String> = cases
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    for name in [
        "root_flags",
        "version_types",
        "deep_chain_300",
        "entries",
        "lz4",
        "lzma",
        "meta_flag_u32",
        "lz4_offset_0_match",
        "lz4_match_in_the_last_literals",
        "lzma_end_marker_corrupt",
        "lzma_declared_smaller",
    ] {
        assert!(names.iter().any(|n| n == name), "{name}");
    }
    // UnityPy loads the offset-0 match and the short LZMA size, and raises on
    // the last-literals match and the corrupt end marker (rabex's decoders
    // did the opposite of each).
    assert_eq!(case("lz4_offset_0_match")["load"], J::Null);
    assert_eq!(case("lzma_declared_smaller")["load"], J::Null);
    assert!(case("lz4_match_in_the_last_literals")["load"].is_string());
    assert!(case("lzma_end_marker_corrupt")["load"].is_string());
    let refused: Vec<&str> = cases.iter().filter_map(|c| c["refused"].as_str()).collect();
    for what in [
        "decoded payload",
        "versions",
        "classes",
        "class entries",
        "common-string versions",
        "common-string indices",
        "nodes",
        "strings",
        "brotli",
    ] {
        assert!(refused.contains(&what), "no refused case for {what}");
    }
    // A request 3: UnityPy keeps a TPK node's u32 meta flag positive.
    let high = &case("meta_flag_u32")["classes"][0]["outcomes"][0]["rows"];
    assert_eq!(high[0][8].as_i64(), Some(0x8000_4000));
    let flags = case("root_flags");
    let classes: Vec<i64> = flags["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["class"].as_i64().unwrap())
        .collect();
    for id in [10, 11, 20, 21, 30, 31, 40, 41] {
        assert!(classes.contains(&id), "{id}");
    }
    let types: Vec<u64> = case("version_types")["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v.as_str().unwrap().parse::<u64>().unwrap() >> 8) & 0xFF)
        .collect();
    for t in [0, 1, 2, 3, 4, 5, 6, 255] {
        assert!(types.contains(&t), "type byte {t}");
    }
}

/// UnityPy builds the 300-deep chain; with the default limits (a depth of
/// 256) the provider refuses it with a `TreeDepth` limit error naming the
/// first depth past the limit (the root's is 1), from the node list and the
/// tree alike, never a wrapped level or a shorter tree (a safety divergence,
/// plan 2.4's F3).
#[test]
fn a_tree_deeper_than_the_limit_is_an_error_not_a_wrap() {
    let case = case("deep_chain_300");
    let bytes = hex(case["tpk"].as_str().unwrap());
    let v = version(&case["versions"][0]);
    for depth in [256, 299] {
        let provider = TpkProvider::from_bytes(&bytes, &limits(depth)).unwrap();
        let want = LimitError {
            kind: LimitKind::TreeDepth,
            value: u64::from(depth) + 1,
            max: u64::from(depth),
        };
        let list = provider.node_list(1, &v).unwrap_err();
        assert_eq!(list.limit, Some(want), "depth {depth}: {list}");
        assert_eq!(&*list.detail, want.to_string(), "depth {depth}");
        let req = TreeRequest {
            class_id: 1,
            version: &v,
            script: None,
        };
        let tree = provider.tree(&req).unwrap_err();
        assert_eq!(tree.limit, Some(want), "depth {depth}: {tree}");
    }
    let provider = TpkProvider::from_bytes(&bytes, &limits(300)).unwrap();
    let list = provider.node_list(1, &v).unwrap().unwrap();
    let levels: Vec<i32> = list.rows().iter().map(|r| r.level).collect();
    assert_eq!(levels, (0..300).collect::<Vec<i32>>());
}

/// UnityPy's `NODES_CACHE` holds one tree per entry; the provider answers
/// every version that picks one entry with the same `Arc`, and another entry
/// with another.
#[test]
fn two_versions_that_pick_one_entry_share_one_node_list() {
    let provider = open(&case("entries")).unwrap();
    let at = |major: u64, minor: u64| {
        UnityVersion::from_packed((major << 48) | (minor << 32) | (3 << 8) | 1)
    };
    let a = provider.node_list(50, &at(2017, 0)).unwrap().unwrap();
    let b = provider.node_list(50, &at(2017, 3)).unwrap().unwrap();
    let c = provider.node_list(50, &at(2019, 0)).unwrap().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert!(!Arc::ptr_eq(&a, &c));
    // 2018 picks the entry that removes the class: no tree, and no stepping
    // back to the 2017 entry.
    assert!(provider.node_list(50, &at(2018, 0)).unwrap().is_none());
}

#[test]
fn the_provider_is_named_tpk_and_takes_no_script_requests() {
    let provider = open(&case("root_flags")).unwrap();
    assert_eq!(provider.name(), "tpk");
    assert!(!provider.accepts_scripts());
}

/// Every request maps to a tree exactly where UnityPy builds a node list,
/// and two versions that pick one entry get one `Arc<Tree>` (plan 3.12).
#[test]
fn the_provider_answers_trees_where_unitypy_builds_them_and_shares_each() {
    for case in synthetic() {
        if case["load"].is_string() || case.get("refused").is_some() {
            continue;
        }
        let provider = open(&case).unwrap();
        for record in case["classes"].as_array().unwrap() {
            let class_id = i32::try_from(record["class"].as_i64().unwrap()).unwrap();
            for v in case["versions"].as_array().unwrap() {
                let v = version(v);
                let req = TreeRequest {
                    class_id,
                    version: &v,
                    script: None,
                };
                let list = provider.node_list(class_id, &v);
                match (provider.tree(&req), list) {
                    (Ok(Some(tree)), Ok(Some(list))) => assert!(Arc::ptr_eq(tree.list(), &list)),
                    (Ok(None), Ok(None)) | (Err(_), Err(_)) => {}
                    (tree, list) => panic!("class {class_id}: tree {tree:?}, node list {list:?}"),
                }
            }
        }
    }
    let provider = open(&case("entries")).unwrap();
    let at = |minor: u64| UnityVersion::from_packed((2017 << 48) | (minor << 32) | (3 << 8) | 1);
    let (v0, v3) = (at(0), at(3));
    let ask = |v| {
        provider
            .tree(&TreeRequest {
                class_id: 50,
                version: v,
                script: None,
            })
            .unwrap()
            .unwrap()
    };
    assert!(Arc::ptr_eq(&ask(&v0), &ask(&v3)));
}

/// The Python that runs the oracle, if this is the oracle image.
fn python() -> Option<String> {
    let candidates = std::env::var("UNITYRS_PYTHON").map_or_else(
        |_| vec!["python3".to_owned(), "python".to_owned()],
        |p| vec![p],
    );
    candidates.into_iter().find(|p| {
        Command::new(p)
            .args(["-c", "import UnityPy"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// G7 on UnityPy's own TPK (module docs).
#[test]
fn image_tpk() {
    let required = std::env::var_os("UNITYRS_ORACLE_TPK").map(PathBuf::from);
    let python = python();
    let tpk = match (&required, &python) {
        (Some(path), _) => path.clone(),
        (None, Some(python)) => {
            let out = Command::new(python)
                .args(["-c", "import os, UnityPy; print(os.path.join(os.path.dirname(UnityPy.__file__), 'resources', 'lzma.tpk'))"])
                .output()
                .unwrap();
            assert!(out.status.success());
            PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        }
        (None, None) => {
            eprintln!(
                "image_tpk: skipped, not in the oracle image (no Python with UnityPy, UNITYRS_ORACLE_TPK unset)"
            );
            return;
        }
    };
    let len = std::fs::metadata(&tpk).map_or(0, |m| m.len());
    assert!(len > 0, "{}: missing or empty", tpk.display());
    // UnityPy's outcomes: the job's file, or one the image's Python writes
    // now (removed when the test ends).
    let made = if std::env::var_os("UNITYRS_ORACLE_TPK_OUTCOMES").is_some() {
        None
    } else {
        assert!(
            required.is_none(),
            "UNITYRS_ORACLE_TPK is set without UNITYRS_ORACLE_TPK_OUTCOMES"
        );
        let python = python.unwrap();
        let made = tempfile(&format!("unityrs-g7-{}.jsonl", std::process::id()));
        let status = Command::new(&python)
            .arg(root().join("tools/fixtures/tpk.py"))
            .arg("--outcomes")
            .arg(&made.0)
            .arg("--tpk")
            .arg(&tpk)
            .status()
            .unwrap();
        assert!(status.success(), "tools/fixtures/tpk.py --outcomes failed");
        Some(made)
    };
    let outcomes = made.as_ref().map_or_else(
        || PathBuf::from(std::env::var_os("UNITYRS_ORACLE_TPK_OUTCOMES").unwrap()),
        |m| m.0.clone(),
    );
    let text = std::fs::read_to_string(&outcomes)
        .unwrap_or_else(|e| panic!("{}: {e}", outcomes.display()));
    assert!(!text.is_empty(), "{}: empty", outcomes.display());
    let records: Vec<J> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let provider = TpkProvider::open(&tpk).unwrap();
    let pairs = compare(&provider, &records[0], records[1..].iter());
    let class_count = provider.class_count();
    // UnityPy 1.25.2's TPK: 409 classes, 1,334 versions (`tpk.py --survey`).
    assert!(
        class_count > 300 && pairs > 300_000,
        "{class_count} classes, {pairs} pairs"
    );
    eprintln!(
        "image_tpk: {class_count} classes, {pairs} (class, version) pairs, every outcome UnityPy's"
    );
}

/// A file removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tempfile(name: &str) -> TempFile {
    TempFile(std::env::temp_dir().join(name))
}
