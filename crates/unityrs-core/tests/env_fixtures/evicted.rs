//! The testkit's dependency scenes (`env::dependency_scenes`), whose loads
//! UnityPy made with `load_file(..., is_dependency=True)` where the scene
//! says so and the core with `Environment::load_dependency`, replayed under
//! both modes and three option sets: `EnvOptions::unitypy()`, `batch()`, and
//! `batch()` with a pool so small that every dependency load evicts the one
//! before (plan section 9, Q13, revised after review round 6: under `Lazy`
//! a nested container of an evicted dependency that a later load's entry
//! dropped was never reached, so the container pass missed the paths its
//! tables set in another file).

use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, LoadMode, Source};
use unityrs_core::{FileContext, RawStr};
use unityrs_testkit::env::dependency_scenes;

use super::run::{Run, load, ops};

/// The recorded dependency scenes: each one's name and steps.
fn recorded() -> Vec<(String, Vec<J>)> {
    let doc = load("env.json").expect("tests/fixtures/d/env.json");
    doc["dependency_scenes"]
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

/// The option sets: UnityPy's (nothing evicted), the batch engine's, and
/// the batch engine's with a one-byte pool (every dependency load evicts
/// the dependency loaded before it).
fn option_sets() -> Vec<(&'static str, EnvOptions)> {
    let batch = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    let mut evicting = batch.clone();
    evicting.dep_cache_bytes = 1;
    vec![
        ("unitypy", EnvOptions::unitypy()),
        ("batch", batch),
        ("evicting", evicting),
    ]
}

/// The core's steps for one recorded dependency scene: its loads, a
/// dependency's through `Environment::load_dependency`, then the steps
/// `Run` takes.
fn replay(name: &str, steps: &[J], mode: LoadMode, options: EnvOptions) -> (Vec<J>, u64) {
    let (_, d) = dependency_scenes()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap();
    let run = Run::with(d.scene.clone(), mode, options);
    let mut out = Vec::new();
    for op in ops(steps) {
        if op[0] == "load_dependency" {
            let i = usize::try_from(op[1].as_u64().unwrap()).unwrap();
            let (file, data) = &d.scene.files[i];
            let loaded = run.env.load_dependency(Source::Bytes {
                name: RawStr::new(file.clone()),
                data: unityrs_core::Bytes::from(data.clone()),
            });
            let mut s = run.state();
            s["op"] = op.clone();
            s["result"] = json!(if loaded.is_ok() { "ok" } else { "error" });
            out.push(s);
        } else {
            out.push(run.step(&op));
        }
    }
    (out, run.env.stats().dependencies_evicted)
}

/// The keys of a step's state without the nested containers' own names
/// (plan 3.13, (b)).
fn keys(step: &J) -> Vec<J> {
    step["cabs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|k| k.as_str() != Some("inner"))
        .cloned()
        .collect()
}

/// Every step's result and `objects` are UnityPy's under each mode and
/// option set, and the keys at the end are UnityPy's once the nested
/// container's own name is out (plan 3.13, (b)); the evicting options do
/// evict. `files` is not compared: `load_dependency` lists nothing there,
/// where UnityPy's `load_file` lists a dependency (`PARITY.md` section
/// 19.3). Under `Lazy` with the evicting options the dropped `INNER` is an
/// evicted dependency's, which only the container pass reaches.
#[test]
fn dependency_scenes_answer_as_unitypy_whatever_the_pool_evicts() {
    let all = recorded();
    assert_ne!(all.len(), 0, "no dependency scene was recorded");
    for (name, steps) in all {
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            for (label, options) in option_sets() {
                let (ours, evictions) = replay(&name, &steps, mode, options);
                if label == "evicting" {
                    assert!(evictions > 0, "{name} {mode:?}: nothing was evicted");
                }
                for (want, got) in steps.iter().zip(&ours) {
                    let op = &want["op"];
                    for part in ["result", "objects"] {
                        assert_eq!(got[part], want[part], "{name} {mode:?} {label} {op} {part}");
                    }
                }
                let (want, got) = (steps.last().unwrap(), ours.last().unwrap());
                assert_eq!(keys(got), keys(want), "{name} {mode:?} {label}: the keys");
            }
        }
    }
}
