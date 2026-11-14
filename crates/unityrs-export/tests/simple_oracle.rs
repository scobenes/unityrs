//! Replay the pinned L7 recording once the coordinator generates it.
#![allow(clippy::format_collect, reason = "test protocol hex encoding")]
#[path = "support/simple_cases.rs"]
mod cases;
use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::simple;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[test]
fn every_simple_export_matches_the_pinned_oracle() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple_export.json");
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "L7 recording pending: run tools/fixtures/simple_export.py in the pinned oracle, then --check"
            );
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let recorded: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(recorded["reference"], "UnityPy 1.25.2");
    assert_eq!(recorded["level"], "L7");
    let recorded = recorded["cases"].as_array().unwrap();
    let cases = cases::cases();
    assert_eq!(recorded.len(), cases.len());
    for (case, oracle) in cases.into_iter().zip(recorded) {
        assert_eq!(oracle["case"], case.name);
        assert_eq!(oracle["class_id"], case.class_id);
        assert_eq!(oracle["extension"], case.extension);
        assert_eq!(
            oracle["input"],
            hex(&case.input),
            "stale input {}",
            case.name
        );
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(case.input),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let output = simple::export(
            &file.object(1).unwrap(),
            &env,
            case.extension,
            &Meter::unlimited(),
        )
        .unwrap();
        let mut files = serde_json::Map::new();
        if let Some(output) = output {
            files.insert(
                format!("payload{}", output.extension),
                hex(output.bytes.as_slice()).into(),
            );
        }
        assert_eq!(
            serde_json::Value::Object(files),
            oracle["files"],
            "{}",
            case.name
        );
    }
}
