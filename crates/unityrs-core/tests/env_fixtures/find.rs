//! `Environment::find_file` against UnityPy's `find_file`: the scenes of
//! `unityrs_testkit::env::find_scenes`, recorded by `tools/fixtures/env.py`.

use serde_json::{Value as J, json};
use unityrs_core::env::LoadMode;
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

/// `find_file` returns what UnityPy's returns for every kind of file it
/// finds, step for step with UnityPy's state, under both load modes
/// (review round 11: a bundle the resolver loaded was `None`, the answer
/// a miss gives): a `SerializedFile` and a raw reader from the registry (a
/// bundle entry's, a top-level file's), each the very object `env.files`
/// or `env.cabs` holds; from the resolver a bundle, a `SerializedFile` and
/// a raw reader, each listed under the resolved path, a dependency but the
/// reader (which UnityPy gives no `is_dependency`); a CAB a resolver load
/// registered; a second ask of a resolver file, now the registry's; a name
/// nothing answers (the walk's `FileNotFoundError`, the resolver's
/// `find_file`); and a bundle with `is_dependency=False`, whose objects
/// join `env.objects`. Last, a WebFile, which UnityPy returns and P1
/// refuses as `load` does (plan section 1, Q7), changing nothing.
#[test]
fn find_file_returns_what_unitypy_returns() {
    let scenes = find_scenes();
    let recorded = recorded_finds();
    assert_eq!(recorded.len(), scenes.len());
    let refused = json!(error_kind(&Error::Unsupported(Unsupported::WebFile)));
    for (name, steps) in recorded {
        let (_, scene) = scenes.iter().find(|(n, _)| *n == name).unwrap();
        let kinds: Vec<&J> = steps.iter().map(|s| &s["result"][0]).collect();
        for kind in ["serialized", "bundle", "raw", "web"] {
            assert!(kinds.contains(&&json!(kind)), "{name}: no {kind} found");
        }
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let run = Run::new(scene.scene.clone(), mode);
            let mut before = J::Null;
            for want in &steps {
                let got = run.step(&want["op"]);
                if want["result"][0] == "web" {
                    assert_eq!(got["result"], refused, "{name} {mode:?}");
                    assert_eq!(state(&got), before, "{name} {mode:?}: the refusal kept");
                } else {
                    assert_eq!(&got, want, "{name} {mode:?}");
                }
                before = state(&got);
            }
        }
    }
}
