//! Replay real Shader.export calls recorded from hand-built `SerializedFiles`.
use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::shader;

fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2));
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => panic!("invalid fixture hex"),
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

#[test]
fn every_shader_export_matches_the_pinned_oracle() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    assert_eq!(recorded["reference"], "UnityPy 1.25.2");
    assert_eq!(recorded["tier"], "A");
    let cases = recorded["cases"].as_array().unwrap();
    assert!(cases.len() >= 159, "shader oracle coverage shrank");
    let mut names = std::collections::HashSet::new();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        assert!(names.insert(name), "duplicate fixture case {name}");
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic-shader.assets"),
                data: Bytes::from(unhex(case["input"].as_str().unwrap())),
            })
            .unwrap_or_else(|error| panic!("{name}: {error}"))
        else {
            panic!("{name}: expected SerializedFile")
        };
        let result = shader::export(&file.object(1).unwrap(), &env, &Meter::unlimited());
        let oracle = &case["oracle"];
        if let Some(text) = oracle.get("text") {
            let output = result.unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                output.as_slice(),
                text.as_str().unwrap().as_bytes(),
                "{name}"
            );
        } else {
            let Err(error) = result else {
                panic!("{name}: expected oracle exception {oracle}")
            };
            if oracle["exception"] == "LZ4BlockError" && error.exception().is_none() {
                assert!(matches!(error, shader::Error::Core(_)), "{name}: {error}");
                assert!(
                    error.to_string().contains("LZ4BlockError"),
                    "{name}: {error}"
                );
            } else {
                assert_eq!(error.exception(), oracle["exception"].as_str(), "{name}");
            }
            if matches!(
                error.exception(),
                Some(
                    "ValueError"
                        | "NotImplementedError"
                        | "IndexError"
                        | "LZ4BlockError"
                        | "TypeError"
                        | "AttributeError"
                )
            ) {
                assert_eq!(
                    error.to_string(),
                    oracle["message"].as_str().unwrap(),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn shader_blob_boundaries_match_the_pinned_oracle() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    let cases = recorded["blobs"].as_array().unwrap();
    assert!(cases.len() >= 60, "shader blob oracle coverage shrank");
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let data = unhex(case["input"].as_str().unwrap());
        let major = u32::try_from(case["version"][0].as_u64().unwrap()).unwrap();
        let minor = u32::try_from(case["version"][1].as_u64().unwrap()).unwrap();
        let index = case["index"].as_i64().unwrap();
        let meter = Meter::unlimited();
        let result = shader::Program::parse(&data, (major, minor), &meter)
            .and_then(|program| program.export(index, &meter));
        let oracle = &case["oracle"];
        if let Some(text) = oracle.get("text") {
            let output = result.unwrap_or_else(|error| panic!("blob {name}: {error}"));
            assert_eq!(
                output.as_slice(),
                text.as_str().unwrap().as_bytes(),
                "blob {name}"
            );
        } else {
            let Err(error) = result else {
                panic!("blob {name}: expected oracle exception {oracle}")
            };
            assert_eq!(
                error.exception(),
                oracle["exception"].as_str(),
                "blob {name}"
            );
            if matches!(
                error.exception(),
                Some("ValueError" | "NotImplementedError" | "IndexError" | "LZ4BlockError")
            ) {
                assert_eq!(
                    error.to_string(),
                    oracle["message"].as_str().unwrap(),
                    "blob {name}"
                );
            }
        }
    }
}

#[test]
fn source_block_output_cap_is_preserved() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    let mut context = unityrs_core::FileContext::unitypy();
    context.limits.max_block_output = 1;
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(context);
    let env = Environment::new(options);
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("synthetic.assets"),
            data: Bytes::from(unhex(recorded["cases"][0]["input"].as_str().unwrap())),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let error = shader::export(&file.object(1).unwrap(), &env, &Meter::unlimited()).unwrap_err();
    let shader::Error::Core(unityrs_core::Error::Limit(limit)) = error else {
        panic!("expected typed block cap refusal: {error}")
    };
    assert_eq!(limit.kind(), unityrs_core::error::LimitKind::BlockOutput);
    assert_eq!(limit.max(), 1);
}
