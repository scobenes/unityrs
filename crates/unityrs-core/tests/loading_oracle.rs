//! L0 keys and CABs, L1 object counts, L2 object values from the pinned
//! UnityPy recorder. The coordinator generates this fixture in the oracle
//! container; missing JSON is a hard failure there, and a reported skip
//! during native development.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/loading.rs"]
mod support;
use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::value::Value;
use unityrs_core::{Parsed, RawStr};

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn value(p: &Parsed, v: &Value) -> J {
    match v {
        Value::Str(s) => json!(s.to_str().unwrap()),
        Value::Dict(m) => p
            .entries(m)
            .map(|(k, v)| (std::str::from_utf8(k).unwrap().to_owned(), value(p, v)))
            .collect(),
        _ => panic!("unexpected synthetic field {v:?}"),
    }
}
fn summary(env: &Environment, prefix: &str) -> J {
    let objects: Vec<_> = env
        .objects()
        .map(|obj| {
            let obj = obj.unwrap();
            let p = obj.read_typetree(None, true).unwrap();
            json!({"path_id":obj.path_id(),"tree":value(&p,p.value())})
        })
        .collect();
    json!({"keys":env.files().map(|e| e.name.to_str().unwrap().strip_prefix(prefix).unwrap_or(e.name.to_str().unwrap()).to_owned()).collect::<Vec<_>>(),
        "cabs":env.registry().keys().map(|k| k.to_str().unwrap().to_owned()).collect::<Vec<_>>(),
        "object_count":objects.len(),"objects":objects})
}

#[test]
fn loading_replays_unitypy_l0_l1_l2() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/loading/oracle.json");
    if !path.exists() {
        assert!(
            std::env::var_os("CI").is_none(),
            "generate tools/fixtures/loading.py in the oracle container"
        );
        eprintln!("SKIP: loading oracle fixture awaits coordinator generation");
        return;
    }
    let fixture: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        fixture["inputs"],
        support::inputs(),
        "testkit inputs changed"
    );
    for (input, scene) in fixture["inputs"]["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(fixture["scenes"].as_array().unwrap())
    {
        let env = Environment::new(EnvOptions::unitypy());
        env.load_zip_file(Source::Bytes {
            name: RawStr::from("synthetic.zip"),
            data: unhex(input["zip"].as_str().unwrap()).into(),
        })
        .unwrap();
        assert_eq!(summary(&env, ""), scene["reference"], "{}", scene["name"]);
        if !scene["name"].as_str().unwrap().contains("split") && scene["name"] != "missing-zero" {
            assert_eq!(scene["zip"], scene["reference"]);
        }
    }
    let s = unityrs_testkit::scratch::Scratch::new("oracle-splits");
    let bytes = unhex(fixture["inputs"]["asset"].as_str().unwrap());
    let half = bytes.len() / 2;
    std::fs::write(s.path().join("CAB-S.split0"), &bytes[..half]).unwrap();
    std::fs::write(s.path().join("CAB-S.split1"), &bytes[half..]).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    env.load_files([s.path().join("CAB-S.split1"), s.path().join("CAB-S.split0")])
        .unwrap();
    assert_eq!(
        summary(&env, &format!("{}/", s.path().display())),
        fixture["filesystem_split"]
    );
    for name in ["CAB-P.split0", "CAB-P.split0.bak", "CAB-M.split1"] {
        std::fs::write(s.path().join(name), &bytes).unwrap();
    }
    std::fs::create_dir(s.path().join("dir.split1")).unwrap();
    std::fs::write(s.path().join("dir.split1/inner"), &bytes).unwrap();
    for case in fixture["filesystem_cases"].as_array().unwrap() {
        let env = Environment::new(EnvOptions::unitypy());
        let result = env.load_file(Source::Path(s.path().join(case["path"].as_str().unwrap())));
        if case["reference"].get("error").is_some() {
            assert!(result.is_err());
        } else {
            result.unwrap();
            assert_eq!(
                summary(&env, &format!("{}/", s.path().display())),
                case["reference"]
            );
        }
    }
}

#[test]
fn review_rounds_replay_cpython_zip_and_missing_path_cases() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/loading/oracle.json");
    if !path.exists() {
        assert!(
            std::env::var_os("CI").is_none(),
            "generate loading.py in the oracle container"
        );
        eprintln!("SKIP: loading oracle fixture awaits coordinator generation");
        return;
    }
    let fixture: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let Some(cases) = fixture["review_cases"].as_array() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "regenerate loading.py for round one"
        );
        eprintln!("SKIP: round one oracle cases await coordinator generation");
        return;
    };
    for case in cases {
        let env = Environment::new(EnvOptions::unitypy());
        let result = env.load_zip_file(Source::Bytes {
            name: RawStr::from("review.zip"),
            data: unhex(case["zip"].as_str().unwrap()).into(),
        });
        if matches!(case["name"].as_str(), Some("method-12" | "method-14")) {
            assert!(case["oracle"].get("error").is_none()); // Tier B: UnityPy loads these.
            assert!(result.is_err());
        } else if case["oracle"].get("error").is_some() {
            assert!(result.is_err(), "{}", case["name"]);
        } else {
            result.unwrap_or_else(|e| panic!("{}: {e:?}", case["name"]));
            assert_eq!(summary(&env, ""), case["oracle"], "{}", case["name"]);
        }
    }
    let s = unityrs_testkit::scratch::Scratch::new("review-oracle-missing");
    let asset = unhex(fixture["inputs"]["asset"].as_str().unwrap());
    std::fs::write(s.path().join("CAB-P.split0"), asset).unwrap();
    for case in fixture["missing_paths"].as_array().unwrap() {
        let env = Environment::new(EnvOptions::unitypy());
        let path = s.path().join(case["path"].as_str().unwrap());
        let result = match case["caller"].as_str().unwrap() {
            "constructor" => env.load_path(&path),
            "load_file" => env.load_file(Source::Path(path)).map(|_| ()),
            "load_files" => env.load_files([path]),
            _ => panic!("unknown caller"),
        };
        if case["oracle"].get("error").is_some() {
            assert!(result.is_err());
        } else {
            result.unwrap();
            assert_eq!(
                summary(&env, &format!("{}/", s.path().display())),
                case["oracle"],
                "{case}"
            );
        }
    }
}

#[test]
fn explicit_zip_split_path_replays_unitypy() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/loading/oracle.json");
    let fixture = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<J>(&s).ok());
    let Some(case) = fixture.as_ref().and_then(|v| v.get("explicit_zip_split")) else {
        assert!(
            std::env::var_os("CI").is_none(),
            "regenerate loading.py for round two"
        );
        eprintln!("SKIP: round two oracle cases await coordinator generation");
        return;
    };
    let s = unityrs_testkit::scratch::Scratch::new("explicit-zip-split-oracle");
    for (i, part) in case["parts"].as_array().unwrap().iter().enumerate() {
        std::fs::write(
            s.path().join(format!("x.split{i}")),
            unhex(part.as_str().unwrap()),
        )
        .unwrap();
    }
    let env = Environment::new(EnvOptions::unitypy());
    env.load_zip_file(Source::Path(s.path().join("x.split0")))
        .unwrap();
    assert_eq!(summary(&env, ""), case["oracle"]);
}
