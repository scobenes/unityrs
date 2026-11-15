use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_testkit::{
    ngui::{Object, V, s, st, vector},
    scene3d::{Layout, file},
};

fn load(value: V) -> (Environment, unityrs_core::ObjectRef) {
    let input = file(Layout::Unity2022, &[], &[Object::new(1, 83, value)]).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(f) = env
        .load(Source::Bytes {
            name: RawStr::from("audio.assets"),
            data: Bytes::from(input),
        })
        .unwrap()
    else {
        panic!("file")
    };
    (env, f.object(1).unwrap())
}
#[test]
fn inline_precedes_resource_and_accepts_legacy_byte_arrays() {
    for data in [
        V::Bytes(b"OggSpayload".to_vec()),
        vector(
            V::UInt8(0),
            b"OggSpayload".iter().map(|&x| V::UInt8(x)).collect(),
        ),
    ] {
        let (env, obj) = load(st(
            "AudioClip",
            vec![
                ("m_Name", s("clip")),
                ("m_Type", V::Int(14)),
                ("m_AudioData", data),
                (
                    "m_Resource",
                    st(
                        "StreamedResource",
                        vec![
                            ("m_Source", s("missing.resS")),
                            ("m_Offset", V::UInt64(0)),
                            ("m_Size", V::UInt64(4)),
                        ],
                    ),
                ),
            ],
        ));
        let m = Meter::unlimited();
        let files = unityrs_export::audio::export(&obj, &env, true, &m).unwrap();
        assert_eq!(files.as_slice()[0].name.as_slice(), b"clip.ogg");
        assert_eq!(files.as_slice()[0].bytes.as_slice(), b"OggSpayload");
        drop(files);
        assert_eq!(m.used(), 0);
    }
}
#[test]
fn streamed_range_and_empty_resource() {
    for (offset, size, expected) in [
        (3, 8, b"RIFFdata".as_slice()),
        (u64::MAX, 0, b"".as_slice()),
    ] {
        let (env, obj) = load(st(
            "AudioClip",
            vec![
                ("m_Name", s("stream")),
                ("m_AudioData", V::Bytes(vec![])),
                (
                    "m_Resource",
                    st(
                        "StreamedResource",
                        vec![
                            ("m_Source", s("audio.resS")),
                            ("m_Offset", V::UInt64(offset)),
                            ("m_Size", V::UInt64(size)),
                        ],
                    ),
                ),
            ],
        ));
        env.load(Source::Bytes {
            name: RawStr::from("audio.resS"),
            data: Bytes::from_static(b"preRIFFdatapost"),
        })
        .unwrap();
        let m = Meter::unlimited();
        let files = unityrs_export::audio::export(&obj, &env, true, &m).unwrap();
        if size == 0 {
            assert!(files.is_empty());
        } else {
            assert_eq!(files.as_slice()[0].bytes.as_slice(), expected);
        }
        drop(files);
        assert_eq!(m.used(), 0);
    }
}

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn replay_pinned_audio_data_sources() {
    use unityrs_export::{audio, simple};

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-audio/tests/fixtures/audio.json"
    ))
    .unwrap();
    let cases = fixture["sources"]
        .as_array()
        .expect("coordinator: regenerate audio.json with python3 tools/fixtures/audio.py");
    assert_eq!(cases.len(), 10, "replay every source case");
    for case in cases {
        let label = case["case"].as_str().unwrap();
        let mut fields = vec![("m_Name", s(case["name"].as_str().unwrap()))];
        if let Some(data) = case["data"].as_str() {
            let data = hex(data);
            let value = if case["legacy_array"] == true {
                vector(V::UInt8(0), data.into_iter().map(V::UInt8).collect())
            } else {
                V::Bytes(data)
            };
            fields.push(("m_AudioData", value));
        }
        if let Some(resource) = case["resource"].as_object() {
            fields.push((
                "m_Resource",
                st(
                    "StreamedResource",
                    vec![
                        ("m_Source", s(resource["source"].as_str().unwrap())),
                        ("m_Offset", V::UInt64(resource["offset"].as_u64().unwrap())),
                        ("m_Size", V::UInt64(resource["size"].as_u64().unwrap())),
                    ],
                ),
            ));
        }
        let (env, obj) = load(st("AudioClip", fields));
        for (name, data) in case["resources"].as_object().unwrap() {
            env.load(Source::Bytes {
                name: RawStr::from(name.as_str()),
                data: Bytes::from(hex(data.as_str().unwrap())),
            })
            .unwrap();
        }
        let m = Meter::unlimited();
        let result = audio::export(&obj, &env, true, &m);
        match case["rust"].as_str().unwrap() {
            "ok" => {
                let files = result.unwrap_or_else(|e| panic!("{label}: {e}"));
                let expected = case["oracle"]["files"].as_object().expect("oracle files");
                assert_eq!(files.len(), expected.len(), "{label}");
                for file in files.as_slice() {
                    let name = std::str::from_utf8(file.name.as_slice()).unwrap();
                    assert_eq!(
                        file.bytes.as_slice(),
                        hex(expected[name].as_str().unwrap()),
                        "{label}: {name}"
                    );
                }
            }
            "missing" => {
                assert_eq!(case["oracle"]["error"], "FileNotFoundError", "{label}");
                assert!(
                    matches!(
                        result,
                        Err(audio::Error::Read(simple::Error::Core(
                            unityrs_core::Error::NotFound(unityrs_core::NotFound::Resource(_))
                        )))
                    ),
                    "{label}: {result:?}"
                );
            }
            "neither" => {
                assert_eq!(case["oracle"]["error"], "ValueError", "{label}");
                assert!(
                    matches!(
                        result,
                        Err(audio::Error::Read(simple::Error::Field(
                            "m_AudioData or m_Resource"
                        )))
                    ),
                    "{label}: {result:?}"
                );
            }
            "empty" => {
                // PARITY section 19.2: return before resolution/decoding for size 0.
                assert!(case["oracle"]["error"].is_string(), "{label}");
                assert!(result.unwrap().is_empty(), "{label}");
            }
            other => panic!("unknown source expectation {other}"),
        }
        assert_eq!(m.used(), 0, "{label}");
    }
}
