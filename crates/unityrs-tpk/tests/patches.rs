//! The regression tests of the safety patches unityrs kept in its fork of
//! rabex's TPK reader (F2-F11, `PARITY.md` section 19.3), rewritten for the
//! port of `Tpk.py` that replaced the fork: each pins a behaviour the port
//! keeps and is named after the patch that first gave it.
//!
//! Where UnityPy has an outcome, it comes from `tools/fixtures/tpk.py`'s
//! synthetic set (`tests/fixtures/tpk/synthetic.json`, recorded from
//! UnityPy 1.25.2). Where unityrs differs by design (a bound UnityPy does not
//! have), the TPK is built here, and what is expected is the provider's
//! documented bound.

use std::sync::Arc;

use common::{Blob, V2, assert_load_is_unitypys, limit_of, limits, load, stored, v2};
use serde_json::Value as J;
use unityrs_core::limits::Limits;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::NodeList;
use unityrs_core::{LimitError, LimitKind, RawStr, UnityVersion};
use unityrs_tpk::{TpkError, TpkProvider};

mod common;

// --- UnityPy's outcomes ---------------------------------------------------------

fn cases() -> Vec<J> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tpk/synthetic.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fixture: J = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["unitypy"], "1.25.2");
    fixture["cases"].as_array().unwrap().clone()
}

fn case(name: &str) -> J {
    cases()
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

/// The case's TPK, opened with the tree depth it was recorded with.
fn open(case: &J) -> Result<TpkProvider, TpkError> {
    let depth = u32::try_from(case["depth"].as_u64().unwrap()).unwrap();
    TpkProvider::from_bytes(&hex(case["tpk"].as_str().unwrap()), &limits(depth))
}

/// The case's versions, as UnityPy's integers.
fn versions(case: &J) -> Vec<UnityVersion> {
    case["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| UnityVersion::from_packed(v.as_str().unwrap().parse().unwrap()))
        .collect()
}

/// UnityPy's outcome for `class` at the case's `at`-th version: `{"rows":
/// [...]}`, `{"none": exception}` or `{"error": exception}`.
fn outcome(case: &J, class: i64, at: usize) -> J {
    let record = case["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["class"] == class)
        .unwrap_or_else(|| panic!("{}: no class {class}", case["name"]));
    let pick = usize::try_from(record["picks"][at].as_u64().unwrap()).unwrap();
    record["outcomes"][pick].clone()
}

/// The class UnityPy was asked for and does not hold: the last record.
fn absent(case: &J) -> i64 {
    case["classes"].as_array().unwrap().last().unwrap()["class"]
        .as_i64()
        .unwrap()
}

/// The provider's answer for `class_id` at `v`, as UnityPy's outcome is
/// recorded: its levels for a tree, `none`, or its error; the compiled tree
/// must agree with the node list.
fn answer(
    provider: &TpkProvider,
    class_id: i32,
    v: &UnityVersion,
) -> Result<Option<Vec<i32>>, ProviderError> {
    let tree = provider.tree(&TreeRequest {
        class_id,
        version: v,
        script: None,
    });
    match (provider.node_list(class_id, v), tree) {
        (Ok(Some(list)), Ok(Some(tree))) => {
            assert!(Arc::ptr_eq(&list, tree.list()));
            Ok(Some(levels(&list)))
        }
        (Ok(None), Ok(None)) => Ok(None),
        (Err(list), Err(tree)) => {
            assert_eq!(list, tree, "class {class_id}: the tree's error");
            Err(list)
        }
        (list, tree) => panic!("class {class_id}: node list {list:?}, tree {tree:?}"),
    }
}

/// What UnityPy's outcome says [`answer`] gives: the levels of its tree,
/// `None`, or the name of the exception it raised.
fn expected(outcome: &J) -> Result<Option<Vec<i32>>, String> {
    if let Some(rows) = outcome.get("rows") {
        let levels = rows.as_array().unwrap().iter();
        return Ok(Some(
            levels
                .map(|r| i32::try_from(r[0].as_i64().unwrap()).unwrap())
                .collect(),
        ));
    }
    if outcome.get("none").is_some() {
        return Ok(None);
    }
    Err(outcome["error"]
        .as_str()
        .unwrap_or_else(|| panic!("{outcome}"))
        .to_owned())
}

/// Whether the provider's answer is UnityPy's outcome: the same levels or
/// none, or the error the port gives for UnityPy's exception
/// ([`common::walk_error_is_unitypys`]).
fn is_unitypys(
    got: &Result<Option<Vec<i32>>, ProviderError>,
    want: &Result<Option<Vec<i32>>, String>,
) -> bool {
    match (got, want) {
        (Ok(got), Ok(want)) => got == want,
        (Err(e), Err(python)) => common::walk_error_is_unitypys(python, e),
        _ => false,
    }
}

fn levels(list: &NodeList) -> Vec<i32> {
    list.rows().iter().map(|r| r.level).collect()
}

/// Every class and version of `name` against UnityPy's outcome.
fn replay(name: &str) -> usize {
    let case = case(name);
    let provider = open(&case).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut pairs = 0;
    for record in case["classes"].as_array().unwrap() {
        let class = record["class"].as_i64().unwrap();
        for (at, v) in versions(&case).iter().enumerate() {
            let want = expected(&outcome(&case, class, at));
            let got = answer(&provider, i32::try_from(class).unwrap(), v);
            assert!(
                is_unitypys(&got, &want),
                "{name}: class {class} at {:?}: UnityPy {want:?}, unityrs {got:?}",
                v.packed()
            );
            pairs += 1;
        }
    }
    pairs
}

/// Runs `f` on its own thread and fails the test, clearly, past the
/// deadlock guard (`unityrs_testkit::guard`) if it has not answered: a walk
/// that never ends.
fn without_a_hang<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    unityrs_testkit::guard::finishes("the walk to end", f)
}

// --- F2: an unknown class ---------------------------------------------------------

/// `ClassInformation[class_id]` raises `KeyError` for a class the TPK does
/// not hold, which is no tree.
#[test]
fn f2_an_unknown_class_has_no_tree() {
    let mut checked = 0;
    for case in cases() {
        if !case["load"].is_null() || case.get("refused").is_some() {
            continue;
        }
        let provider = open(&case).unwrap();
        let class = absent(&case);
        let versions = versions(&case);
        for (at, v) in versions.iter().enumerate() {
            assert_eq!(
                outcome(&case, class, at)["none"],
                "KeyError",
                "{}",
                case["name"]
            );
            let class = i32::try_from(class).unwrap();
            assert!(!provider.class_ids().any(|c| c == class));
            assert_eq!(answer(&provider, class, v), Ok(None), "{}", case["name"]);
        }
        checked += usize::from(!versions.is_empty());
    }
    assert!(checked > 20, "{checked} cases");
}

// --- F3: reading as UnityPy reads ------------------------------------------------

/// The header's sizes are unsigned (`<IbbbbIII`): a compressed size past
/// the file, negative as a signed int or not, is UnityPy's "Invalid
/// compressed size", whatever the codec; the uncompressed size is a
/// maximum to LZ4, ignored by LZMA and unread when stored.
#[test]
fn f3_negative_header_sizes_are_errors_and_huge_ones_fail_at_the_end() {
    let mut names = vec![
        "negative_compressed_size".to_owned(),
        "stored_declared_u32_max".to_owned(),
    ];
    for codec in ["stored", "lz4", "lzma"] {
        for size in ["2147483647", "2147483648"] {
            names.push(format!("compressed_size_{codec}_{size}"));
        }
    }
    for size in ["larger", "smaller", "2_31", "u32_max"] {
        names.push(format!("lz4_declared_{size}"));
    }
    for size in ["zero", "smaller", "larger", "u32_max"] {
        names.push(format!("lzma_declared_{size}"));
    }
    let mut loaded = 0;
    for name in &names {
        let case = case(name);
        let got = open(&case);
        assert_load_is_unitypys(name, case["load"].as_str(), &got);
        if let Ok(provider) = got {
            assert_eq!(provider.class_count(), 9, "{name}");
            loaded += 1;
        }
    }
    // Both outcomes are pinned.
    assert!(loaded > 0 && loaded < names.len(), "{loaded}");
}

/// Each count is read as `Tpk.py` reads it: the version list's and the
/// common-string indices' through a struct format (a negative count
/// raises), the others through `range()` (a negative count reads nothing,
/// and its records are read as what follows). None sizes an allocation, so
/// a huge one fails when the bytes run out (or at the provider's bound).
#[test]
fn f3_negative_and_huge_counts_read_as_unitypy_reads_them_and_never_panic() {
    let (mut seen, mut loaded) = (0, 0);
    for case in cases() {
        let name = case["name"].as_str().unwrap().to_owned();
        if !name.starts_with("count_") {
            continue;
        }
        let got = open(&case);
        assert_load_is_unitypys(&name, case["load"].as_str(), &got);
        if let Ok(provider) = got {
            let count = case["class_count"].as_u64().unwrap();
            assert_eq!(
                u64::try_from(provider.class_count()).unwrap(),
                count,
                "{name}"
            );
            replay(&name);
            loaded += 1;
        }
        seen += 1;
    }
    // Seven counts, each at -1, -2^31, 2^30 and 2^31 - 1, and at -1 with its
    // records written anyway; both outcomes are pinned.
    assert_eq!(seen, 7 * 5);
    assert!(loaded > 0 && loaded < seen, "{loaded}");
    // The two counts UnityPy reads as a struct format raise at -1.
    for name in ["count_versions_-1", "count_indices_-1"] {
        assert!(open(&case(name)).is_err(), "{name}");
    }
}

/// Every flag byte loads as UnityPy loads it, each root read under its own
/// flag; and the blob cut anywhere is an error, never a panic.
#[test]
fn f3_every_flag_byte_reads_without_a_panic() {
    assert!(replay("flag_bytes") >= 256);
    let case = case("flag_bytes");
    let file = hex(case["tpk"].as_str().unwrap());
    let payload = &file[20..];
    for end in 0..payload.len() {
        let got = TpkProvider::from_bytes(&stored(&payload[..end]), &Limits::default());
        assert!(got.is_err(), "cut at {end}");
    }
}

/// A node or string id past its table (or a release root past the nodes)
/// is UnityPy's `IndexError`: an error, not "no tree".
#[test]
fn f3_a_node_or_string_id_past_its_table_is_an_error() {
    let case = case("ids_past_their_tables");
    for class in [70, 71, 72] {
        assert!(outcome(&case, class, 0).get("error").is_some(), "{class}");
    }
    assert!(replay("ids_past_their_tables") >= 4);
}

/// A node that lists itself, or its grandparent, is a walk UnityPy never
/// ends; the provider's ends with an error.
#[test]
fn f3_a_node_that_lists_itself_or_its_grandparent_is_an_error_not_a_hang() {
    for nodes in [vec![vec![0]], vec![vec![1], vec![2], vec![0]]] {
        let provider = load(&Blob::class_1(nodes), &Limits::default()).unwrap();
        let got = without_a_hang(move || provider.node_list(1, &v2()).map(|_| ()));
        assert!(got.is_err());
    }
}

/// Levels are `i32`s: a 300-deep chain gives levels 0 to 299, as UnityPy's
/// (rabex's reader held them in a `u8`).
#[test]
fn f3_a_300_deep_chain_walks_past_the_255_levels_a_u8_held() {
    let case = case("deep_chain_300");
    let want = expected(&outcome(&case, 1, 0)).unwrap().unwrap();
    assert_eq!(want, (0..300).collect::<Vec<i32>>());
    assert_eq!(replay("deep_chain_300"), 2);
}

/// A string whose length runs past the payload, which UnityPy decodes
/// short, is an error; one that ends the payload reads whole.
#[test]
fn f3_a_string_longer_than_the_data_is_an_error() {
    let refused = case("refused_string_past_the_payload");
    assert!(refused["load"].is_null(), "UnityPy loads it");
    match open(&refused) {
        Err(TpkError::Malformed(why)) => assert!(why.contains("longer than"), "{why}"),
        got => panic!("{got:?}"),
    }
    // One byte short, and exact.
    let (full, at) = Blob::class_1(vec![vec![]]).write();
    for (tail, ok) in [(&b"ab"[..], false), (&b"abc"[..], true)] {
        let mut payload = full[..at.strings].to_vec();
        payload.extend_from_slice(&1_i32.to_le_bytes());
        payload.push(3);
        payload.extend_from_slice(tail);
        let got = TpkProvider::from_bytes(&stored(&payload), &Limits::default());
        assert_eq!(got.is_ok(), ok, "{tail:?}: {got:?}");
    }
    assert!(replay("string_length_5_bytes") > 0);
}

/// `get_item_for_version` scans a class's entries forward in their stored
/// order, keeping the last at or below the request and stopping at the
/// first above it. (rabex's version of this test pinned the same scan in
/// `TpkCommonString.GetCount`, which nothing in UnityPy 1.25.2 calls and
/// the port does not have.)
#[test]
fn f3_an_entry_is_picked_by_a_forward_scan_as_unitypy_does() {
    let case = case("entries");
    let provider = open(&case).unwrap();
    let versions = versions(&case);
    // Class 51 lists 2019, 2017, 2021: at 2018 the scan stops at 2019.
    let at = |year: u64| {
        let packed = (year << 48) | (3 << 8) | 1;
        versions
            .iter()
            .position(|v| v.packed_u64() == Some(packed))
            .unwrap()
    };
    assert_eq!(outcome(&case, 51, at(2018))["none"], "ValueError");
    assert_eq!(answer(&provider, 51, &versions[at(2018)]), Ok(None));
    let same = |a, b| {
        let list = |y| provider.node_list(51, &versions[at(y)]).unwrap().unwrap();
        Arc::ptr_eq(&list(a), &list(b))
    };
    assert!(same(2019, 2020));
    assert!(!same(2020, 2021));
    assert!(replay("entries") > 0);
}

// --- F4 and F9: the walk ----------------------------------------------------------

/// A class's entries, each picked as UnityPy picks it and each release
/// tree's rows as UnityPy builds them; an editor root alone is no tree; the
/// class ids are the dict's, a class listed twice once.
#[test]
fn f4_a_class_lists_its_entries_and_each_release_tree_as_rows() {
    let case = case("entries");
    let provider = open(&case).unwrap();
    let mut ids: Vec<i32> = case["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| i32::try_from(r["class"].as_i64().unwrap()).unwrap())
        .filter(|&id| i64::from(id) != absent(&case))
        .collect();
    ids.sort_unstable();
    assert_eq!(provider.class_ids().collect::<Vec<_>>(), ids);
    assert_eq!(provider.class_count(), ids.len());
    replay("entries");
    // An editor root alone (flags 64), at the entry's version: AssertionError.
    let roots = self::case("root_flags");
    assert_eq!(outcome(&roots, 20, 1)["none"], "AssertionError");
    replay("root_flags");
    // Every field of a row, the meta flag kept unsigned.
    replay("node_fields");
    replay("meta_flag_u32");
}

#[test]
fn f4_a_300_deep_chain_gives_levels_0_to_299_when_the_depth_allows_and_an_error_when_not() {
    let case = case("deep_chain_300");
    let bytes = hex(case["tpk"].as_str().unwrap());
    let v = &versions(&case)[0];
    let provider = TpkProvider::from_bytes(&bytes, &limits(300)).unwrap();
    let want = expected(&outcome(&case, 1, 0));
    assert_eq!(answer(&provider, 1, v).unwrap(), want.unwrap());
    for depth in [299, 256, 255, 1] {
        let provider = TpkProvider::from_bytes(&bytes, &limits(depth)).unwrap();
        let e = answer(&provider, 1, v).unwrap_err();
        assert_eq!(
            e.limit.map(|l| l.kind),
            Some(LimitKind::TreeDepth),
            "depth {depth}"
        );
    }
    // A root alone fits a depth of 1: class 50 at 2019 is node 2, a leaf.
    let case = self::case("entries");
    let versions = versions(&case);
    let at = versions
        .iter()
        .position(|v| v.packed_u64() == Some((2019 << 48) | (3 << 8) | 1))
        .unwrap();
    assert_eq!(expected(&outcome(&case, 50, at)), Ok(Some(vec![0])));
    let bytes = hex(case["tpk"].as_str().unwrap());
    let provider = TpkProvider::from_bytes(&bytes, &limits(1)).unwrap();
    assert_eq!(answer(&provider, 50, &versions[at]), Ok(Some(vec![0])));
}

/// Each bound of the walk is its own limit error: the depth (counting the
/// root as 1) of the first node past [`Limits::max_tree_depth`], or the rows
/// walked and still to walk past [`TpkProvider::MAX_TREE_ROWS`].
#[test]
fn f9_the_walk_names_the_bound_it_stops_at() {
    let case = case("deep_chain_300");
    let bytes = hex(case["tpk"].as_str().unwrap());
    let v = &versions(&case)[0];
    for (max, depth) in [(256, 257), (299, 300), (1, 2)] {
        let provider = TpkProvider::from_bytes(&bytes, &limits(max)).unwrap();
        let got = limit_of(&provider.node_list(1, v).unwrap_err());
        let want = LimitError {
            kind: LimitKind::TreeDepth,
            value: depth,
            max: u64::from(max),
        };
        assert_eq!(got, want, "{max}");
    }
    let rows = rows_past_the_bound();
    let got = limit_of(&rows.node_list(1, &v2()).unwrap_err());
    let max = u64::try_from(TpkProvider::MAX_TREE_ROWS).unwrap();
    let want = LimitError {
        kind: LimitKind::Count,
        value: max + 1,
        max,
    };
    assert_eq!(got, want);
    // A node that lists itself reaches the depth first.
    let provider = load(&Blob::class_1(vec![vec![0]]), &limits(256)).unwrap();
    let got = limit_of(&provider.node_list(1, &v2()).unwrap_err());
    assert_eq!(
        (got.kind, got.value, got.max),
        (LimitKind::TreeDepth, 257, 256)
    );
}

/// A node that lists itself or its grandparent, with any depth allowed:
/// the depth or the rows stop the walk.
#[test]
fn f4_a_node_that_lists_itself_or_its_grandparent_is_an_error_not_a_hang() {
    for nodes in [vec![vec![0]], vec![vec![1], vec![2], vec![0]]] {
        for depth in [256, u32::MAX] {
            let provider = load(&Blob::class_1(nodes.clone()), &limits(depth)).unwrap();
            let got = without_a_hang(move || provider.node_list(1, &v2()).map(|_| ()));
            let kind = limit_of(&got.unwrap_err()).kind;
            let want = if depth == 256 {
                LimitKind::TreeDepth
            } else {
                LimitKind::Count
            };
            assert_eq!(kind, want, "depth {depth}");
        }
    }
}

/// A root whose 65,535 sub-nodes each list one more: 2 rows walked and
/// 65,535 still to walk when the second's child is counted.
fn rows_past_the_bound() -> TpkProvider {
    let n = u16::MAX;
    let blob = Blob::class_1(vec![vec![1; usize::from(n)], vec![2], vec![]]);
    load(&blob, &limits(u32::MAX)).unwrap()
}

/// The row bound counts the rows still to walk: a root with 65,535 leaf
/// sub-nodes is 65,536 rows, at the bound, and reads; one more row to walk
/// is refused before it is walked. Each part of the count matters: the rows
/// walked, the rows waiting on the stack, and the sub-nodes of the node
/// being walked, which alone pass the bound when the root's first sub-node
/// lists 65,535 leaves and its second waits (65,538 rows).
#[test]
fn f4_the_row_bound_counts_the_rows_still_to_walk() {
    let blob = Blob::class_1(vec![vec![1; usize::from(u16::MAX)], vec![]]);
    let provider = load(&blob, &Limits::default()).unwrap();
    let list = provider.node_list(1, &v2()).unwrap().unwrap();
    assert_eq!(list.rows().len(), TpkProvider::MAX_TREE_ROWS);
    assert!(rows_past_the_bound().node_list(1, &v2()).is_err());
    let blob = Blob::class_1(vec![vec![1, 2], vec![2; usize::from(u16::MAX)], vec![]]);
    let provider = load(&blob, &Limits::default()).unwrap();
    let max = u64::try_from(TpkProvider::MAX_TREE_ROWS).unwrap();
    let want = LimitError {
        kind: LimitKind::Count,
        value: max + 2,
        max,
    };
    assert_eq!(limit_of(&provider.node_list(1, &v2()).unwrap_err()), want);
}

// --- F5: LZMA ---------------------------------------------------------------------

/// An LZMA TPK reads as its payload: the same answers as the stored one
/// (`decompress_lzma` runs to the end of the stream; the header's size is
/// not used).
#[test]
fn f5_an_lzma_tpk_reads_as_its_payload_not_behind_zeros() {
    let stored = open(&case("root_flags")).unwrap();
    for name in ["lzma", "lzma_declared_zero", "lzma_declared_larger", "lz4"] {
        let case = case(name);
        let provider = open(&case).unwrap();
        assert_eq!(
            provider.class_ids().collect::<Vec<_>>(),
            stored.class_ids().collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(provider.versions(), stored.versions(), "{name}");
        for class in stored.class_ids() {
            for v in stored.versions() {
                assert_eq!(
                    answer(&provider, class, v),
                    answer(&stored, class, v),
                    "{name}"
                );
            }
        }
        assert!(replay(name) > 0);
    }
}

// --- F6 and F7: flags and versions ------------------------------------------------

/// Classes 10, 20, 30 and 40 have the root flags 0, 64, 128 and 192; each
/// is followed by a class with both roots (11, 21, 31, 41), which reads
/// wrong when the one before it is misread.
#[test]
fn f6_each_root_is_read_under_its_own_flag() {
    let case = case("root_flags");
    for (class, release) in [(10, false), (20, false), (30, true), (40, true), (45, true)] {
        let want = outcome(&case, class, 1);
        assert_eq!(want.get("rows").is_some(), release, "{class}");
    }
    for class in [11, 21, 31, 41] {
        assert_eq!(expected(&outcome(&case, class, 1)), Ok(Some(vec![0, 1, 1])));
    }
    assert!(replay("root_flags") >= 9 * 3);
}

/// Versions whose type byte is 0-5, 6 and 255, in the version list, a
/// class's entries and the common strings: each loads and orders as its
/// integer, as UnityPy's `UnityVersion(int)` does.
#[test]
fn f7_a_version_of_any_type_byte_loads_and_orders_as_its_integer() {
    let case = case("version_types");
    let provider = open(&case).unwrap();
    assert_eq!(provider.versions(), &versions(&case)[..]);
    assert!(replay("version_types") > 20);
}

/// The versions a file's request is parsed from compare as UnityPy's
/// packed integers (`UnityVersion.from_str`: `major << 48 | minor << 32 |
/// build << 16 | type << 8 | number`, the type letters a, b, c, f, p, x
/// being 0-5), and so as their fields do: a request picks the TPK entry
/// UnityPy's integer comparison picks.
#[test]
fn f7_packing_orders_versions_as_their_fields_did() {
    let versions: [(&str, u64); 8] = [
        ("5.6.7f1", (5 << 48) | (6 << 32) | (7 << 16) | (3 << 8) | 1),
        ("2018.4.2a3", (2018 << 48) | (4 << 32) | (2 << 16) | 3),
        (
            "2018.4.2b1",
            (2018 << 48) | (4 << 32) | (2 << 16) | (1 << 8) | 1,
        ),
        (
            "2018.4.2f1",
            (2018 << 48) | (4 << 32) | (2 << 16) | (3 << 8) | 1,
        ),
        (
            "2018.4.2p2",
            (2018 << 48) | (4 << 32) | (2 << 16) | (4 << 8) | 2,
        ),
        (
            "2018.4.3f1",
            (2018 << 48) | (4 << 32) | (3 << 16) | (3 << 8) | 1,
        ),
        ("2022.1.0f1", (2022 << 48) | (1 << 32) | (3 << 8) | 1),
        ("6000.0.50f1", (6000 << 48) | (50 << 16) | (3 << 8) | 1),
    ];
    let parse = |s: &str| UnityVersion::parse(&RawStr::from(s)).unwrap();
    for (a, pa) in versions {
        assert_eq!(parse(a), UnityVersion::from_packed(pa), "{a}");
        for (b, pb) in versions {
            assert_eq!(parse(a).cmp(&parse(b)), pa.cmp(&pb), "{a} {b}");
        }
    }
    // root_flags lists its classes at 2018.4.2f1: a request parsed from
    // 2018.4.2b1 is below it (ValueError: no tree), 2018.4.2p2 above.
    let provider = open(&case("root_flags")).unwrap();
    assert_eq!(answer(&provider, 31, &parse("2018.4.2b1")), Ok(None));
    assert_eq!(
        answer(&provider, 31, &parse("2018.4.2f1")),
        Ok(Some(vec![0, 1, 1]))
    );
    assert_eq!(
        answer(&provider, 31, &parse("2018.4.2p2")),
        Ok(Some(vec![0, 1, 1]))
    );
}

// --- F8: the tables ---------------------------------------------------------------

/// Each table at [`TpkProvider::MAX_RECORDS`] reads; one record more is
/// refused as that table's when its count is read, before any record (none
/// follow here); the entries of every class count as one table.
#[test]
fn f8_each_table_is_bounded_before_its_records_are_read() {
    let max = TpkProvider::MAX_RECORDS;
    let n = u16::try_from(max - 1).unwrap();
    let blob = Blob {
        versions: vec![V2; max],
        classes: (0..i32::try_from(max).unwrap())
            .map(|id| (id, vec![(V2, 0)]))
            .collect(),
        common: vec![(V2, 1); max],
        indices: vec![0; max],
        nodes: (0..max)
            .map(|i| if i == 0 { vec![n] } else { vec![] })
            .collect(),
        strings: max,
    };
    let provider = load(&blob, &Limits::default()).unwrap();
    assert_eq!(provider.class_count(), max);
    assert_eq!(provider.versions().len(), max);
    assert_eq!(
        provider.node_list(7, &v2()).unwrap().unwrap().rows().len(),
        2
    );
    let (bytes, at) = blob.write();
    let over = i32::try_from(max + 1).unwrap();
    for (table, at) in [
        ("versions", at.versions),
        ("classes", at.classes),
        ("common-string versions", at.common),
        ("common-string indices", at.indices),
        ("nodes", at.nodes),
        ("strings", at.strings),
    ] {
        let mut payload = bytes[..at].to_vec();
        payload.extend_from_slice(&over.to_le_bytes());
        let got = TpkProvider::from_bytes(&stored(&payload), &Limits::default());
        let bound = u64::try_from(max).unwrap();
        match got {
            Err(TpkError::TooLarge {
                what,
                value,
                max: m,
            }) => {
                assert_eq!((what, value, m), (table, bound + 1, bound));
            }
            got => panic!("{table}: {got:?}"),
        }
    }
    // The entries of every class: max - 1 and 1 fit; max - 1 and 2 do not.
    let mut blob = Blob::class_1(vec![vec![]]);
    blob.classes = vec![(1, vec![(V2, 0); max - 1]), (2, vec![(V2, 0)])];
    assert!(load(&blob, &Limits::default()).is_ok());
    blob.classes[1].1.push((V2, 0));
    match load(&blob, &Limits::default()) {
        Err(TpkError::TooLarge { what, value, .. }) => {
            assert_eq!(
                (what, value),
                ("class entries", u64::try_from(max).unwrap() + 1)
            );
        }
        got => panic!("{got:?}"),
    }
}
