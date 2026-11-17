//! Replay real Shader.export calls recorded from hand-built `SerializedFiles`.
use std::fmt::Write;
use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::shader;

#[path = "support/shader_code.rs"]
mod shader_code;

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

fn no_programs_text(cases: &[serde_json::Value]) -> unityrs_core::limits::MeteredVec<u8> {
    // The specified 5.5+ correction is the existing converter with no programs.
    // This case has the same parsed form and no platforms, and already pins it.
    let no_programs = cases
        .iter()
        .find(|c| c["case"] == "empty_platforms")
        .unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("no-programs.assets"),
            data: Bytes::from(unhex(no_programs["input"].as_str().unwrap())),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let output = shader::export(&file.object(1).unwrap(), &env, &Meter::unlimited()).unwrap();
    assert_eq!(
        output.as_slice(),
        no_programs["oracle"]["text"].as_str().unwrap().as_bytes()
    );
    output
}

#[test]
fn every_shader_export_matches_the_pinned_oracle() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    assert_eq!(recorded["reference"], "UnityPy 1.25.2");
    assert_eq!(recorded["tier"], "A");
    let cases = recorded["cases"].as_array().unwrap();
    let no_programs_text = no_programs_text(cases);
    assert!(cases.len() >= 159, "shader oracle coverage shrank");
    let mut names = std::collections::HashSet::new();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        assert!(names.insert(name), "duplicate fixture case {name}");
        // E3 replays these same inputs with pinned Tier C text in
        // shader_modern.json, retaining the omitted-program oracle answer.
        if name.starts_with("player_only_") {
            continue;
        }
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
        if name == "converter_invalid_source_utf8" {
            assert_eq!(oracle["exception"], "UnicodeDecodeError");
            assert_eq!(
                result.unwrap().as_slice(),
                [
                    shader::HEADER,
                    concat!(
                        "Shader \"Synthetic/Écho\" {\nProperties {\n\n}\nSubShader {\n",
                        " Pass {\n  GpuProgramID 0\nProgram \"vp\" {\nSubProgram \"openGL \" {\n",
                        "\"//shader disassembly not supported on 1\"\n}\n}\n}\n}\n}"
                    )
                ]
                .concat()
                .as_bytes(),
            );
            continue;
        }
        if matches!(
            name,
            "script_missing"
                | "script_string"
                | "script_empty"
                | "script_utf8"
                | "script_invalid_utf8"
        ) {
            // Tier C: retain UnityPy's answer beside the version-first correction.
            assert_no_blob_oracle(name, oracle);
            assert_eq!(
                result.unwrap().as_slice(),
                no_programs_text.as_slice(),
                "{name}: Tier C divergence"
            );
            continue;
        }
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

fn code_shader_text(platform: &str, programs: &[&str]) -> String {
    let mut expected = [shader::HEADER, "Shader \"Synthetic/Code\" {\nProperties {\n\n}\nSubShader {\n Pass {\n  GpuProgramID 7\nProgram \"vp\" {\n"].concat();
    for program in programs {
        write!(expected, "SubProgram \"{platform} \" {{\n{program}\n}}\n").unwrap();
    }
    expected.push_str("}\n}\n}\n}");
    expected
}

#[test]
fn unreadable_code_falls_back_beside_the_recorded_unitypy_raise() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    for name in [
        "invalid_source_utf8_kind_1",
        "invalid_source_utf8_kind_8",
        "invalid_source_utf8_kind_26",
        "invalid_source_utf8_kind_30",
        "metal_short_header",
        "metal_unterminated",
        "metal_offset_100",
        "metal_invalid_source_utf8",
    ] {
        let case = recorded["blobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["case"] == name)
            .unwrap();
        let blob = unhex(case["input"].as_str().unwrap());
        let kind = i32::from_le_bytes(blob[20..24].try_into().unwrap());
        let expected_exception = if name == "metal_short_header" {
            "error"
        } else if matches!(name, "metal_unterminated" | "metal_offset_100") {
            "Exception"
        } else {
            "UnicodeDecodeError"
        };
        assert_eq!(case["oracle"]["exception"], expected_exception, "{name}");
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("code.assets"),
                data: Bytes::from(shader_code::asset(kind, &blob, 1)),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        let platform = match kind {
            1 => "openGL",
            8 => "glcore",
            23 => "metal",
            _ => "xbox360",
        };
        let comment = format!("\"//shader disassembly not supported on {kind}\"");
        let output = shader::export_parsed(&parsed, (2021, 3), &Meter::unlimited()).unwrap();
        assert_eq!(
            output.as_slice(),
            code_shader_text(platform, &[&comment]).as_bytes(),
            "{name}"
        );
        let error = shader::export_with_cap_unitypy_properties(
            &parsed,
            (2021, 3),
            &Meter::unlimited(),
            u64::MAX,
        )
        .unwrap_err();
        assert_eq!(
            error.exception(),
            Some(expected_exception),
            "binding {name}"
        );
        if expected_exception == "Exception" {
            assert_eq!(
                error.to_string(),
                case["oracle"]["message"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn legacy_replacements_fall_back_while_bindings_keep_code_raises() {
    use unityrs_core::Value;
    use unityrs_testkit::bundle::{TestCodec, compress};

    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader_legacy.json")).unwrap();
    for minor in [3, 4] {
        let name = format!("big_endian_5_{minor}_valid");
        let case = recorded["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["case"] == name)
            .unwrap();
        for (kind, code, exception) in [
            (1i32, b"\xff".as_slice(), "UnicodeDecodeError"),
            (23, b"ABCDunterminated", "Exception"),
        ] {
            // Reuse the legacy builder's BE table and revision-specific header.
            // Only the program kind, code length and code bytes change.
            let mut blob = unhex(case["replacement_blob"].as_str().unwrap());
            assert_eq!(blob.len(), 40);
            blob[16..20].copy_from_slice(&kind.to_be_bytes());
            blob[36..40].copy_from_slice(&i32::try_from(code.len()).unwrap().to_be_bytes());
            blob.extend_from_slice(code);
            while !blob.len().is_multiple_of(4) {
                blob.push(0);
            }
            let env = Environment::new(EnvOptions::unitypy());
            let File::Serialized(file) = env
                .load(Source::Bytes {
                    name: RawStr::from("legacy-code.assets"),
                    data: Bytes::from(unhex(case["input"].as_str().unwrap())),
                })
                .unwrap()
            else {
                panic!("file")
            };
            let mut parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
            assert!(
                parsed
                    .replace_scalar(
                        &[b"m_SubProgramBlob"],
                        Value::Bytes(Bytes::from(compress(TestCodec::Lz4, &blob))),
                    )
                    .unwrap()
            );
            assert!(
                parsed
                    .replace_scalar(
                        &[b"decompressedSize"],
                        Value::Int(i64::try_from(blob.len()).unwrap()),
                    )
                    .unwrap()
            );
            let meter = Meter::unlimited();
            let output = shader::export_parsed(&parsed, (5, minor), &meter).unwrap();
            assert_eq!(
                output.as_slice(),
                format!(
                    "{}\"//shader disassembly not supported on {kind}\"\n",
                    shader::HEADER
                )
                .as_bytes(),
                "5.{minor}, kind {kind}"
            );
            drop(output);
            assert_eq!(meter.used(), 0);
            let error =
                shader::export_with_cap_unitypy_properties(&parsed, (5, minor), &meter, u64::MAX)
                    .unwrap_err();
            assert_eq!(error.exception(), Some(exception), "5.{minor}, kind {kind}");
            if kind == 23 {
                assert_eq!(error.to_string(), "String not terminated");
            }
            drop(error);
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn code_fallback_is_local_and_preserves_metal_window_and_slice_rules() {
    // These synthetic expectations come from section 19.2's comment rule;
    // they do not claim new oracle recordings. Existing recordings above
    // pin the raises and shader_blob_boundaries pins Python's slice results.
    for kind in [1, 2, 3, 4, 5, 6, 7, 8, 23, 24, 26, 27, 28, 29, 30] {
        let platform = match kind {
            1 => "openGL",
            2..=4 => "gles3",
            5 => "gles",
            6..=8 => "glcore",
            23 | 24 => "metal",
            _ => "xbox360",
        };
        let metal = matches!(kind, 23 | 24);
        let valid = if metal {
            b"ABCDentry\0source".as_slice()
        } else {
            b"source"
        };
        let mut cases = vec![(vec![0xff], true, "")];
        if metal {
            cases.clear();
            for len in 1..4 {
                cases.push((vec![b'a'; len], true, ""));
            }
            for len in 4..8 {
                let mut code = 0xf00d_cafeu32.to_le_bytes().to_vec();
                code.resize(len, 1);
                cases.push((code, true, ""));
            }
            for len in [0, 1, 32766, 32767, 32768] {
                let mut code = b"ABCD".to_vec();
                code.extend(vec![b'n'; len]);
                cases.push((code, len <= 32767, "n"));
            }
            cases.extend([
                (b"ABCDentry\0\xff".to_vec(), true, ""),
                (b"ABCD\xff\0source".to_vec(), false, "source"),
                (vec![], false, ""),
                (
                    [0xf00d_cafeu32.to_le_bytes(), (-32768i32).to_le_bytes()]
                        .concat()
                        .into_iter()
                        .chain(b"main\0source".iter().copied())
                        .collect(),
                    false,
                    "e",
                ),
            ]);
        }
        for (code, fallback, source) in cases {
            let blob = shader_code::program(kind, &[valid, &code, valid]);
            let env = Environment::new(EnvOptions::unitypy());
            let File::Serialized(file) = env
                .load(Source::Bytes {
                    name: RawStr::from("variants.assets"),
                    data: Bytes::from(shader_code::asset(kind, &blob, 3)),
                })
                .unwrap()
            else {
                panic!("file")
            };
            let output =
                shader::export(&file.object(1).unwrap(), &env, &Meter::unlimited()).unwrap();
            let middle = if fallback {
                format!("\"//shader disassembly not supported on {kind}\"")
            } else {
                format!("\"{source}\"")
            };
            assert_eq!(
                output.as_slice(),
                code_shader_text(platform, &["\"source\"", &middle, "\"source\""]).as_bytes(),
                "kind {kind}, code length {}",
                code.len()
            );
        }
    }
}

fn assert_no_blob_oracle(name: &str, oracle: &serde_json::Value) {
    match name {
        "script_empty" => assert_eq!(oracle["text"], shader::HEADER),
        "script_utf8" => {
            assert_eq!(oracle["text"], shader::HEADER.to_owned() + "// Shader É\n");
        }
        "script_invalid_utf8" => assert_eq!(oracle["exception"], "UnicodeDecodeError"),
        _ => {
            assert_eq!(oracle["exception"], "TypeError", "{name}");
            let message = if name == "script_string" {
                "string argument without an encoding"
            } else {
                "cannot convert 'NoneType' object to bytes"
            };
            assert_eq!(oracle["message"], message, "{name}");
        }
    }
}
