//! `files` is a dict by rank (`listing.rs`'s `Files`; review round 16):
//! whatever order loads list their entries in, from whichever thread, it
//! holds what UnityPy's `env.files` holds when the same loads are made one
//! after the other in the order of their numbers, which the registry's
//! keys follow too (plan section 3.16, R1). The expected values are
//! UnityPy's own `env.files` after every set of the loads
//! (`tools/fixtures/env_race.py`'s `listings_case`, in `env_threads.json`;
//! review round 16's follow-up: they were a hand-written model of a Python
//! dict).

use std::sync::Arc;

use serde_json::{Value as J, json};

use super::registry::open_source;
use super::{EnvOptions, Environment, Source};
use crate::files::File;
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::listings;

/// UnityPy's `env.files` after the listings of every non-empty set of
/// loads (`env_threads.json`'s `listings`): the files listed, each load's
/// number its index, the ones loaded as dependencies, and for each set its
/// `files` (each key, the load whose file it holds and its
/// `is_dependency`), its `objects`' path ids and rule 4's answer for `x`
/// and `y` (the load, or `null`).
fn recorded() -> J {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/d/env_threads.json"
    );
    let doc: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["listings"].clone()
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Every order of `0..n`, by Heap's algorithm.
fn orders(n: usize) -> Vec<Vec<usize>> {
    fn heap(k: usize, a: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if k <= 1 {
            out.push(a.clone());
            return;
        }
        for i in 0..k - 1 {
            heap(k - 1, a, out);
            let j = if k.is_multiple_of(2) { i } else { 0 };
            a.swap(j, k - 1);
        }
        heap(k - 1, a, out);
    }
    let mut out = Vec::new();
    heap(n, &mut (0..n).collect(), &mut out);
    out
}

/// The testkit's six listings (`unityrs_testkit::env::listings`: `x`,
/// `y`, `X`, `x`, `y` and a dependency's `x`, so a key is listed again by a
/// later load and by an earlier one, after it moved, and two keys lower to
/// one name), in every order they can arrive in, each checked after every
/// listing against UnityPy's `env.files` for the loads listed so far:
/// `files` lists each key at its place with its file and that file's
/// `is_dependency`, `objects` steps into the files that are no dependency
/// in that order (each holds one object, of path id its load + 1), and
/// rule 4's lookup by a lowered name answers the file UnityPy's
/// `PPtr.deref` resolves to (`classes/PPtr.py:72-75`, the first key that
/// lowers to it). Each listing comes bounded, so a cursor that does not
/// advance fails instead of looping.
#[test]
fn files_holds_unitypys_dict_whatever_order_the_loads_list_in() {
    let scene = listings();
    let record = recorded();
    let pairs: Vec<J> = scene
        .scene
        .files
        .iter()
        .map(|(n, d)| json!([String::from_utf8_lossy(n), hex(d)]))
        .collect();
    assert_eq!(
        record["files"],
        json!(pairs),
        "the builder changed: run tools/fixtures/env.py"
    );
    assert_eq!(record["dependencies"], json!(scene.dependencies));
    let sets = record["sets"].as_array().unwrap();
    assert_eq!(sets.len(), (1 << scene.scene.files.len()) - 1);
    let want_of = |loads: &[u64]| -> &J {
        let mut sorted = loads.to_vec();
        sorted.sort_unstable();
        sets.iter()
            .find(|s| s["loads"] == json!(sorted))
            .unwrap_or_else(|| panic!("UnityPy has no run of the loads {sorted:?}"))
    };
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::unlimited();
    let files: Vec<File> = (0_u64..)
        .zip(&scene.scene.files)
        .map(|(load, (_, data))| {
            let source = Source::Bytes {
                name: RawStr::from(format!("f{load}").as_str()),
                data: Bytes::from(data.clone()),
            };
            open_source(&RawStr::from("f"), &source, &meter, &ctx).unwrap()
        })
        .collect();
    let load_of = |f: &File| -> u64 {
        let File::Serialized(sf) = f else {
            panic!("a listed file is a SerializedFile");
        };
        let at = files
            .iter()
            .position(|g| matches!(g, File::Serialized(g) if Arc::ptr_eq(g, sf)))
            .expect("a listed file");
        u64::try_from(at).unwrap()
    };
    let all = orders(scene.scene.files.len());
    assert_eq!(all.len(), 720);
    for order in &all {
        let env = Environment::new(EnvOptions::unitypy());
        let mut loads = Vec::new();
        for &i in order {
            let load = u64::try_from(i).unwrap();
            let key = RawStr::new(scene.scene.files[i].0.clone());
            let dependency = scene.dependencies.contains(&i);
            env.list(key, files[i].clone(), dependency, load, &meter)
                .unwrap();
            loads.push(load);
            let want = want_of(&loads);
            let what = format!("order {order:?}, after {loads:?}");
            let bound = want["files"].as_array().unwrap().len() + 1;
            let got: Vec<J> = env
                .files()
                .take(bound)
                .map(|e| {
                    json!([
                        String::from_utf8_lossy(e.name.as_bytes()),
                        load_of(&e.file),
                        e.is_dependency
                    ])
                })
                .collect();
            assert_eq!(json!(got), want["files"], "{what}: files");
            let ids: Vec<i64> = env
                .objects()
                .take(bound)
                .map(|o| o.unwrap().path_id())
                .collect();
            assert_eq!(json!(ids), want["objects"], "{what}: objects");
            for name in ["x", "y"] {
                let got_match = env.files_match(&RawStr::from(name)).map(|f| load_of(&f));
                assert_eq!(
                    json!(got_match),
                    want["rule4"][name],
                    "{what}: rule 4's {name}"
                );
            }
        }
    }
}
