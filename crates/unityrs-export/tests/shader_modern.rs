//! Divergence replays keep UnityPy's recorded answer beside our expected text.
use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::shader;

#[path = "support/shader_int.rs"]
mod shader_int;

#[test]
fn int_properties_follow_shaderlab_integer_default_rules() {
    // Tier C reference: Unity 2021.3's SL-Properties manual, Integer declaration.
    // Finite integral defaults are decimal integers; others keep Python repr.
    // Preserve attributes and ignored flags.
    // No type-5 recording exists in shader{,_modern,_legacy}.json; these are
    // synthetic expectations, not a claim of a new UnityPy oracle observation.
    // f32 represents every integer through 2^24, then only even integers up to
    // 2^25. Use exact neighbours, plus values too large for an i64/i128 cast.
    let defaults = [
        1.,
        -3.,
        0.,
        -0.,
        1.25,
        0.1,
        1e-5,
        16_777_215.,
        16_777_216.,
        16_777_218.,
        -16_777_218.,
        1e20,
        f32::MAX,
        f32::MIN,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let expected = concat!(
        "Shader \"Synthetic/Int\" {\nProperties {\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 1\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = -3\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 0\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 0\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 1.25\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 0.10000000149011612\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 9.999999747378752e-06\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 16777215\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 16777216\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 16777218\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = -16777218\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 100000002004087734272\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = 340282346638528859811704183484516925440\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = -340282346638528859811704183484516925440\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = nan\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = inf\n\n",
        "[HDR] [Toggle(X)] _Count (\"Count é\", Integer) = -inf\n\n",
        "}\n}",
    );
    for blob in [false, true] {
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("int.assets"),
                data: Bytes::from(shader_int::shader_with_properties(5, &defaults, blob)),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        for version in [(2021, 1), (2021, 3), (6000, 3)] {
            let meter = Meter::unlimited();
            let output = shader::export_parsed(&parsed, version, &meter).unwrap();
            assert_eq!(
                output.as_slice(),
                [shader::HEADER, expected].concat().as_bytes()
            );
            drop(output);
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn plain_string_scripts_before_55_are_utf8() {
    use unityrs_testkit::{
        ngui::{Object, V, s, st},
        scene3d::{self, Layout},
    };
    for script in ["", "Shader \"Écho/雪\" {}\nGpuProgramIndex 0\n"] {
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("plain.assets"),
                data: Bytes::from(
                    scene3d::file(
                        Layout::Unity2022,
                        &[],
                        &[Object::new(
                            1,
                            48,
                            st(
                                "Shader",
                                vec![
                                    ("m_Script", s(script)),
                                    ("m_SubProgramBlob", V::Bytes(vec![])),
                                ],
                            ),
                        )],
                    )
                    .unwrap(),
                ),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        for version in [(2, 6), (5, 2), (5, 3), (5, 4)] {
            let result = shader::export_parsed(&parsed, version, &Meter::unlimited()).unwrap();
            assert_eq!(
                result.as_slice(),
                [shader::HEADER, script].concat().as_bytes()
            );
        }
    }
}

#[test]
fn no_blob_paths_follow_the_tier_c_spec() {
    use unityrs_core::Value;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == "script_string")
        .unwrap();
    let input: Vec<u8> = case["input"]
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("empty.assets"),
            data: Bytes::from(input),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let mut parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
    let expected = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == "empty_platforms")
        .unwrap()["oracle"]["text"]
        .as_str()
        .unwrap();
    // No programs means even irrelevant malformed platform metadata is unused.
    for field in [
        b"platforms".as_slice(),
        b"offsets",
        b"compressedLengths",
        b"decompressedLengths",
    ] {
        assert!(parsed.replace_scalar(&[field], Value::Int(-1)).unwrap());
    }
    for (index, script) in [
        Value::Str(RawStr::from("Shader \"Écho/雪\" {}\n")),
        Value::Bytes(Bytes::from_static("Shader \"Écho/雪\" {}\n".as_bytes())),
        Value::Bytes(Bytes::from_static(b"\xff")),
        Value::Int(-1),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(parsed.replace_scalar(&[b"m_Script"], script).unwrap());
        for version in [(5, 5), (2019, 4), (2021, 3), (6000, 3)] {
            let meter = Meter::unlimited();
            let result = shader::export_with_cap(&parsed, version, &meter, 0).unwrap();
            assert_eq!(
                result.as_slice(),
                expected.as_bytes(),
                "{version:?}: script {index}"
            );
            drop(result);
            assert_eq!(meter.used(), 0);
        }
        // The exact lower boundary still uses the retained script, even with
        // a valid parsed form present.
        if index < 2 {
            let result = shader::export_parsed(&parsed, (5, 4), &Meter::unlimited()).unwrap();
            assert_eq!(
                result.as_slice(),
                [shader::HEADER, "Shader \"Écho/雪\" {}\n"]
                    .concat()
                    .as_bytes()
            );
        }
    }
}

#[test]
fn modern_shaders_require_a_parsed_form_even_with_a_script() {
    use unityrs_testkit::{
        ngui::{Object, V, s, st, vector},
        scene3d::{self, Layout},
    };
    for script in [
        None,
        Some(s("retained")),
        Some(V::Bytes(b"retained".to_vec())),
    ] {
        for blob in [vec![], vec![1]] {
            let mut fields = vec![
                ("compressedBlob", V::Bytes(blob)),
                ("platforms", vector(V::Int(0), vec![])),
            ];
            if let Some(script) = script.clone() {
                fields.push(("m_Script", script));
            }
            let data = scene3d::file(
                Layout::Unity2022,
                &[],
                &[Object::new(1, 48, st("Shader", fields))],
            )
            .unwrap();
            let env = Environment::new(EnvOptions::unitypy());
            let File::Serialized(file) = env
                .load(Source::Bytes {
                    name: RawStr::from("missing-parsed-form.assets"),
                    data: Bytes::from(data),
                })
                .unwrap()
            else {
                panic!("file")
            };
            let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
            for version in [(5, 5), (2021, 3)] {
                let meter = Meter::unlimited();
                let error = shader::export_parsed(&parsed, version, &meter).unwrap_err();
                assert!(
                    matches!(error, shader::Error::Field("m_ParsedForm")),
                    "{version:?}: {script:?}: {error}"
                );
                assert_eq!(meter.used(), 0);
            }
        }
    }
}

#[test]
fn modern_layouts_and_refusals_replay_the_recording() {
    replay_recording(None);
}

#[test]
fn player_parameters_follow_their_variant_on_each_platform() {
    replay_recording(Some("player_variant_parameters_per_platform"));
    replay_recording(Some("player_group_parameters_per_platform"));
}

#[test]
fn player_selection_stops_at_the_first_usable_platform() {
    replay_recording(Some("player_first_usable_platform"));
}

#[test]
fn empty_player_groups_emit_no_program_blocks() {
    replay_recording(Some("player_empty_groups_active_False"));
    replay_recording(Some("player_empty_groups_active_True"));
}

fn replay_recording(selected: Option<&str>) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader_modern.json")).unwrap();
    let legacy: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader_legacy.json")).unwrap();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(legacy["cases"].as_array().unwrap())
        .collect();
    assert_eq!(fixture["reference"], "UnityPy 1.25.2");
    assert_eq!(legacy["reference"], "UnityPy 1.25.2");
    assert!(cases.len() >= 100);
    let mut names = std::collections::HashSet::new();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        assert!(names.insert(name), "duplicate case {name}");
        if selected.is_some_and(|selected| selected != name) {
            continue;
        }
        assert!(case["oracle"].get("text").is_some() || case["oracle"].get("exception").is_some());
        let hex = case["input"].as_str().unwrap();
        let bytes: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect();
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(bytes),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let meter = Meter::unlimited();
        let result = shader::export(&file.object(1).unwrap(), &env, &meter);
        let expected = &case["rust"];
        if let Some(text) = expected.get("text") {
            if let Some(replacement) = case.get("replacement_oracle") {
                assert_eq!(
                    text.as_str().unwrap(),
                    shader::HEADER.to_owned() + replacement["text"].as_str().unwrap(),
                    "{name}: direct replacement oracle"
                );
            }
            let output = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                output.as_slice(),
                text.as_str().unwrap().as_bytes(),
                "{name}"
            );
        } else {
            let error = result.unwrap_err();
            let want = expected
                .get("refusal")
                .or_else(|| expected.get("error"))
                .unwrap()
                .as_str()
                .unwrap();
            let matches = match want {
                "BigEndian53Blob" => matches!(
                    error,
                    shader::Error::Unsupported(shader::Unsupported::BigEndian53Blob)
                ),
                "BigEndian54Blob" => matches!(
                    error,
                    shader::Error::Unsupported(shader::Unsupported::BigEndian54Blob)
                ),
                "GpuProgramIndex" => {
                    assert!(matches!(
                        case["replacement_oracle"]["exception"].as_str(),
                        Some("ValueError" | "IndexError")
                    ));
                    matches!(
                        error,
                        shader::Error::Unsupported(shader::Unsupported::GpuProgramIndex)
                    )
                }
                "LayoutIndex" => matches!(error, shader::Error::LayoutIndex),
                "ProgramType" => matches!(error, shader::Error::ProgramType(_)),
                _ => panic!("unknown expected error {want}"),
            };
            assert!(matches, "{name}: {error:?}, expected {want}");
            if expected.get("refusal").is_some() {
                assert_eq!(error.exception(), None);
            }
        }
        assert_eq!(meter.used(), 0, "{name}: retained charge");
    }
    if let Some(selected) = selected {
        assert!(names.contains(selected), "missing case {selected}");
    }
}
