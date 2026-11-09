//! `Environment::find_file` against UnityPy's `find_file`: the scenes of
//! `unityrs_testkit::env::find_scenes`, recorded by `tools/fixtures/env.py`
//! (each scene's loads, the container, every ask, the container again),
//! under every option set: as UnityPy with `keep_resolver_loads`, and by
//! `PARITY.md` section 19.3's row "resolver loads under
//! `keep_resolver_loads` false" alone without it.

use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::FileContext;
use unityrs_core::env::{EnvOptions, LoadMode};
use unityrs_core::error::{Error, Unsupported};
use unityrs_testkit::env::find_scenes;

use super::run::{Run, error_kind, load};

/// The recorded `find_file` scenes: each name and its steps.
fn recorded_finds() -> Vec<(String, Vec<J>)> {
    let doc = load("env.json").expect("tests/fixtures/d/env.json");
    doc["find_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_owned(),
                s["steps"].as_array().unwrap().clone(),
            )
        })
        .collect()
}

/// A step's state: everything but its op and result.
fn state(step: &J) -> J {
    let mut s = step.clone();
    s["op"] = J::Null;
    s["result"] = J::Null;
    s
}

/// The option sets a replay runs under, each named: with
/// `keep_resolver_loads` (`EnvOptions::unitypy()`, and the same with a
/// cache that evicts every dependency it can), or without it
/// (`EnvOptions::batch()`, and the same with a cache that never evicts),
/// each in both load modes.
fn option_sets(keep: bool) -> Vec<(String, EnvOptions)> {
    let mut out = Vec::new();
    for cache in [None, Some(if keep { 1 } else { u64::MAX })] {
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let mut o = if keep {
                EnvOptions::unitypy()
            } else {
                EnvOptions::batch(Arc::new(FileContext::unitypy()))
            };
            o.load = mode;
            if let Some(cache) = cache {
                o.dep_cache_bytes = cache;
            }
            out.push((format!("keep {keep}, cache {cache:?}, {mode:?}"), o));
        }
    }
    out
}

/// What UnityPy's step `want` reads as under options without
/// `keep_resolver_loads` (`PARITY.md` section 19.3, "resolver loads under
/// `keep_resolver_loads` false"): every file `find_file` loads, whatever
/// `is_dependency`, is in none of `files`, `objects` or `container`, so
/// those are what they were after the loads (`after`), the container's
/// rows those recorded then (`rows`), and a found file's `files` key, which
/// only a caller's load can hold now, `None` otherwise; the loads, what
/// `find_file` returns and the CAB registry are UnityPy's.
fn without_keep(want: &J, after: &J, rows: &J) -> J {
    let mut e = want.clone();
    let op = want["op"][0].as_str().unwrap();
    if op == "load" {
        return e;
    }
    e["files"] = after["files"].clone();
    e["objects"] = after["objects"].clone();
    match op {
        "container" => e["result"] = rows.clone(),
        "find_file" => {
            let callers: Vec<&J> = after["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| &f[0])
                .collect();
            if let Some(found) = e["result"].as_array_mut()
                && !callers.contains(&&found[3])
            {
                found[3] = J::Null;
            }
        }
        _ => {}
    }
    e
}

/// Replays every recorded scene under `options` (named `what`), each step
/// against UnityPy's, as `expect` turns it for these options (the step,
/// the step after the loads, the container recorded then). A WebFile,
/// which UnityPy returns and P1 refuses as `load` does (plan section 1,
/// Q7), changes nothing: from it on, UnityPy's state holds the file and
/// the core's is the state before it, so only the results are compared.
fn replay(what: &str, options: &EnvOptions, expect: impl Fn(&J, &J, &J) -> J) {
    let scenes = find_scenes();
    let refused = json!(error_kind(&Error::Unsupported(Unsupported::WebFile)));
    for (name, steps) in recorded_finds() {
        let (_, scene) = scenes.iter().find(|(n, _)| *n == name).unwrap();
        let loaded = steps.iter().rposition(|s| s["op"][0] == "load").unwrap();
        let after = &steps[loaded];
        assert_eq!(steps[loaded + 1]["op"], json!(["container"]), "{name}");
        let rows = &steps[loaded + 1]["result"];
        let run = Run::with(scene.scene.clone(), options.load, options.clone());
        let mut before = J::Null;
        let mut web: Option<J> = None;
        for want in &steps {
            let got = run.step(&want["op"]);
            let want = expect(want, after, rows);
            let at = format!("{name} {what} {}", want["op"]);
            if want["result"][0] == "web" {
                assert_eq!(got["result"], refused, "{at}");
                assert_eq!(state(&got), before, "{at}: the refusal kept");
                web = Some(before.clone());
            } else if let Some(kept) = &web {
                assert_eq!(got["result"], want["result"], "{at}");
                assert_eq!(&state(&got), kept, "{at}: the state before the WebFile");
            } else {
                assert_eq!(got, want, "{at}");
            }
            before = state(&got);
        }
    }
}

/// `find_file` returns what UnityPy's returns for every kind of file it
/// finds, step for step with UnityPy's state, the container's rows
/// included, under both load modes with `keep_resolver_loads`, with a
/// cache that never evicts (`EnvOptions::unitypy()`) and one that evicts
/// every dependency it can (review round 11: a bundle the resolver loaded
/// was `None`, the answer a miss gives): a `SerializedFile` and a raw
/// reader from the registry (a bundle entry's, a top-level file's), each
/// the very object `env.files` or `env.cabs` holds; from the resolver a
/// bundle, a `SerializedFile` and a raw reader, each listed under the
/// resolved path, a dependency but the reader (which UnityPy gives no
/// `is_dependency`); a CAB a resolver load registered; a second ask of a
/// resolver file, now the registry's; a name nothing answers (the walk's
/// `FileNotFoundError`, the resolver's `find_file`); and with
/// `is_dependency=False` a bundle, a `SerializedFile` and a raw reader,
/// whose objects join `env.objects` and whose CABs' rows join
/// `env.container`. Last, a WebFile, which P1 refuses.
#[test]
fn find_file_returns_what_unitypy_returns() {
    let recorded = recorded_finds();
    assert_eq!(recorded.len(), find_scenes().len());
    let kinds: Vec<&J> = recorded
        .iter()
        .flat_map(|(_, steps)| steps.iter().map(|s| &s["result"][0]))
        .collect();
    for kind in ["serialized", "bundle", "raw", "web"] {
        assert!(kinds.contains(&&json!(kind)), "no {kind} found");
    }
    for (what, options) in option_sets(true) {
        replay(&what, &options, |want, _, _| want.clone());
    }
}

/// Without `keep_resolver_loads` (`EnvOptions::batch()`'s options, with
/// its cache and with one that never evicts, in both load modes),
/// `find_file` answers as UnityPy's but for `PARITY.md` section 19.3's row
/// "resolver loads under `keep_resolver_loads` false", exactly: what it
/// loads, a dependency or not, a bundle, a `SerializedFile` or a raw
/// reader, is in none of `files()`, `objects()` or `container()` (review
/// round 12: no row listed this, and since round 11 a load with
/// `is_dependency` false left UnityPy's objects and container rows out
/// too). Each scene's record shows the row is not empty: UnityPy lists
/// resolver loads, and in `find_file_listing` objects and rows, that these
/// options leave out.
#[test]
fn find_file_without_keep_resolver_loads_differs_by_its_row_alone() {
    for (name, steps) in recorded_finds() {
        let loaded = steps.iter().rposition(|s| s["op"][0] == "load").unwrap();
        let last = steps.last().unwrap();
        assert_ne!(
            last["files"], steps[loaded]["files"],
            "{name}: nothing listed"
        );
        if name == "find_file_listing" {
            assert_ne!(last["objects"], steps[loaded]["objects"], "{name}");
            assert_ne!(last["result"], steps[loaded + 1]["result"], "{name}");
        }
    }
    for (what, options) in option_sets(false) {
        replay(&what, &options, without_keep);
    }
}
