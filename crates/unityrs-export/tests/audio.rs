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
fn adpcm_audio_clips_replay_fmod_wavs() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-audio/tests/fixtures/audio.json"
    ))
    .unwrap();
    let hex = |text: &str| -> Vec<u8> {
        text.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect()
    };
    for label in [
        "ima_c2_frames3",
        "fadpcm_c6_frames3",
        "fadpcm_c2_count259_short_frame",
        "fadpcm_c8_count512_short1",
        "ima_c6_count511_short1",
        "ima_c32_limit",
        "fadpcm_c32_limit",
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["case"] == label)
            .unwrap();
        let (env, obj) = load(st(
            "AudioClip",
            vec![
                ("m_Name", s("clip")),
                ("m_AudioData", V::Bytes(hex(case["data"].as_str().unwrap()))),
            ],
        ));
        let m = Meter::unlimited();
        let files = unityrs_export::audio::export(&obj, &env, true, &m).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files.as_slice()[0].name.as_slice(), b"clip.wav");
        assert_eq!(
            files.as_slice()[0].bytes.as_slice(),
            hex(case["oracle"]["files"]["clip.wav"].as_str().unwrap()),
            "{label}"
        );
        drop(files);
        assert_eq!(m.used(), 0);
    }
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
fn replay_pinned_audio_dispatch() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-audio/tests/fixtures/audio.json"
    ))
    .unwrap();
    for label in [
        "ogg",
        "riff",
        "m4a",
        "riff_before_m4a",
        "legacy_ogg",
        "pcm16_c1_r8",
        "named_multi",
        "overrides",
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["case"] == label)
            .unwrap();
        let mut fields = vec![
            ("m_Name", s(case["name"].as_str().unwrap())),
            ("m_AudioData", V::Bytes(hex(case["data"].as_str().unwrap()))),
            // These are the recorder's raw_to_wav defaults. Bank metadata
            // overrides them, and neither format field selects the suffix.
            ("m_Channels", V::Int(2)),
            ("m_Frequency", V::Int(44100)),
        ];
        for (key, value) in [
            ("m_Type", &case["legacy_type"]),
            ("m_CompressionFormat", &case["compression"]),
        ] {
            if let Some(value) = value.as_i64() {
                fields.push((key, V::Int(i32::try_from(value).unwrap())));
            }
        }
        let (env, obj) = load(st("AudioClip", fields));
        let m = Meter::unlimited();
        {
            let files = unityrs_export::audio::export(&obj, &env, true, &m)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            let expected = case["oracle"]["files"].as_object().unwrap();
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
        assert_eq!(m.used(), 0, "{label}");
    }
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

#[test]
#[cfg(feature = "vorbis")]
fn vorbis_audio_clip_uses_the_callers_runtime_provider() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-audio/tests/fixtures/vorbis.json"
    ))
    .unwrap();
    let case = &fixture["cases"][1];
    let hex = |text: &str| {
        text.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
    };
    let (env, obj) = load(st(
        "AudioClip",
        vec![
            ("m_Name", s("clip")),
            ("m_AudioData", V::Bytes(hex(case["bank"].as_str().unwrap()))),
        ],
    ));
    let m = Meter::unlimited();
    assert!(matches!(
        unityrs_export::audio::export(&obj, &env, true, &m),
        Err(unityrs_export::audio::Error::Audio(
            unityrs_audio::Error::VorbisHeadersRequired
        ))
    ));
    let setup = hex(case["setup"].as_str().unwrap());
    let mut table = b"VST1".to_vec();
    for n in [
        1u32,
        u32::try_from(case["crc"].as_u64().unwrap()).unwrap(),
        u32::try_from(setup.len()).unwrap(),
    ] {
        table.extend_from_slice(&n.to_le_bytes());
    }
    table.extend(setup);
    let headers = unityrs_audio::VorbisHeaders::from_bytes(&table, &m).unwrap();
    let files =
        unityrs_export::audio::export_with_vorbis(&obj, &env, true, Some(&headers), &m).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files.as_slice()[0].name.as_slice(), b"clip.wav");
    let wav = files.as_slice()[0].bytes.as_slice();
    assert_eq!(
        wav.len(),
        44 + usize::try_from(case["frames"].as_u64().unwrap() * 4).unwrap()
    );
    assert!(wav[44..].iter().any(|&b| b != 0));
    drop(files);
    drop(headers);
    assert_eq!(m.used(), 0);
}

// Only the independent encoder fixture's own setup packet enters this table.
fn vorbis_fixture() -> (Vec<u8>, Vec<u8>) {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-audio/tests/fixtures/vorbis.json"
    ))
    .unwrap();
    let case = &fixture["cases"][1];
    let setup = hex(case["setup"].as_str().unwrap());
    let mut table = b"VST1".to_vec();
    for n in [
        1u32,
        u32::try_from(case["crc"].as_u64().unwrap()).unwrap(),
        u32::try_from(setup.len()).unwrap(),
    ] {
        table.extend_from_slice(&n.to_le_bytes());
    }
    table.extend(setup);
    (hex(case["bank"].as_str().unwrap()), table)
}

#[derive(Default)]
struct Extracted(std::collections::BTreeMap<Vec<u8>, Vec<u8>>);
impl unityrs_export::extractor::Sink for Extracted {
    fn directory(&mut self, _: &[u8]) -> Result<(), unityrs_export::extractor::Error> {
        Ok(())
    }
    fn file(&mut self, path: &[u8], bytes: &[u8]) -> Result<(), unityrs_export::extractor::Error> {
        self.0.insert(path.to_vec(), bytes.to_vec());
        Ok(())
    }
}

#[test]
#[cfg(feature = "vorbis")]
fn extractor_vorbis_opt_in_writes_decoder_wav_and_none_records_failure_then_continues() {
    use unityrs_export::extractor::{self, Options};

    let (bank, table) = vorbis_fixture();
    let (env, _) = load(st(
        "AudioClip",
        vec![
            ("m_Name", s("clip")),
            ("m_AudioData", V::Bytes(bank.clone())),
        ],
    ));
    env.load(Source::Bytes {
        name: RawStr::from("text.assets"),
        data: Bytes::from(
            file(
                Layout::Unity2022,
                &[],
                &[Object::new(
                    2,
                    49,
                    st(
                        "TextAsset",
                        vec![("m_Name", s("after")), ("m_Script", s("continued"))],
                    ),
                )],
            )
            .unwrap(),
        ),
    })
    .unwrap();
    let m = Meter::unlimited();
    let headers = unityrs_export::VorbisHeaders::from_bytes(&table, &m).unwrap();
    let plan = unityrs_audio::plan(&bank, true, &m).unwrap();
    let decoded = unityrs_audio::decode_with_vorbis(&plan, b"clip", Some(&headers), &m).unwrap();
    for vorbis in [None, Some(&headers)] {
        let mut files = Extracted::default();
        let report = extractor::extract_assets(
            &env,
            &Options {
                use_container: false,
                vorbis,
                ..Options::default()
            },
            &mut files,
            &m,
        )
        .unwrap();
        assert_eq!(files.0[b"after.txt".as_slice()], b"continued");
        if vorbis.is_some() {
            assert!(report.failures.is_empty());
            assert_eq!(report.written.len(), 2);
            assert_eq!(
                files.0[b"clip.wav".as_slice()],
                decoded.as_slice()[0].bytes.as_slice()
            );
        } else {
            assert_eq!(report.written.len(), 1);
            assert_eq!(report.failures.len(), 1);
            assert_eq!(report.failures.as_slice()[0].object.path_id(), 1);
            assert!(matches!(
                report.failures.as_slice()[0].error,
                extractor::Error::Audio(unityrs_export::audio::Error::Audio(
                    unityrs_audio::Error::VorbisHeadersRequired
                ))
            ));
            assert!(!files.0.contains_key(b"clip.wav".as_slice()));
        }
    }
    drop((decoded, plan, headers));
    assert_eq!(m.used(), 0);
}

#[test]
#[cfg(not(feature = "vorbis"))]
fn extractor_without_vorbis_feature_keeps_supplied_table_a_typed_failure() {
    use unityrs_export::extractor::{self, Options};

    let (bank, table) = vorbis_fixture();
    let (env, _) = load(st(
        "AudioClip",
        vec![("m_Name", s("clip")), ("m_AudioData", V::Bytes(bank))],
    ));
    let m = Meter::unlimited();
    let headers = unityrs_export::VorbisHeaders::from_bytes(&table, &m).unwrap();
    let mut files = Extracted::default();
    let report = extractor::extract_assets(
        &env,
        &Options {
            use_container: false,
            vorbis: Some(&headers),
            ..Options::default()
        },
        &mut files,
        &m,
    )
    .unwrap();
    assert!(files.0.is_empty());
    assert!(report.written.is_empty());
    assert_eq!(report.failures.len(), 1);
    assert!(matches!(
        report.failures.as_slice()[0].error,
        extractor::Error::Audio(unityrs_export::audio::Error::Audio(
            unityrs_audio::Error::Unsupported { codec: 15 }
        ))
    ));
    drop((report, headers));
    assert_eq!(m.used(), 0);
}

#[test]
#[cfg(feature = "vorbis")]
fn gameobject_crawl_forwards_vorbis_but_keeps_unitypy_export_options() {
    use unityrs_export::extractor::{self, AssetFilter, Options};
    use unityrs_testkit::ngui::pptr;

    struct GameObjectsOnly;
    impl AssetFilter for GameObjectsOnly {
        fn accepts(&self, obj: &unityrs_core::ObjectRef, _: Option<&unityrs_core::Parsed>) -> bool {
            obj.class_id() == 1
        }
    }

    let (bank, table) = vorbis_fixture();
    let clip = st(
        "AudioClip",
        vec![
            ("m_Name", s("clip")),
            ("m_AudioData", V::Bytes(bank.clone())),
        ],
    );
    let input = file(
        Layout::Unity2022,
        &[],
        &[
            Object::new(
                1,
                1,
                st(
                    "GameObject",
                    vec![
                        ("m_Name", s("root")),
                        (
                            "m_Component",
                            vector(
                                pptr("AudioSource", (0, 0)),
                                vec![pptr("AudioSource", (0, 2))],
                            ),
                        ),
                    ],
                ),
            ),
            Object::new(
                2,
                82,
                st(
                    "AudioSource",
                    vec![
                        ("m_GameObject", pptr("GameObject", (0, 1))),
                        ("m_audioClip", pptr("AudioClip", (0, 3))),
                    ],
                ),
            ),
            Object::new(3, 83, clip),
        ],
    )
    .unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: RawStr::from("scene.assets"),
        data: Bytes::from(input),
    })
    .unwrap();
    let m = Meter::unlimited();
    let headers = unityrs_export::VorbisHeaders::from_bytes(&table, &m).unwrap();
    let plan = unityrs_audio::plan(&bank, true, &m).unwrap();
    let decoded = unityrs_audio::decode_with_vorbis(&plan, b"clip", Some(&headers), &m).unwrap();
    for vorbis in [None, Some(&headers)] {
        let mut files = Extracted::default();
        let report = extractor::extract_assets(
            &env,
            &Options {
                use_container: false,
                export_unknown_as_typetree: true,
                asset_filter: Some(&GameObjectsOnly),
                vorbis,
                ..Options::default()
            },
            &mut files,
            &m,
        )
        .unwrap();
        if vorbis.is_some() {
            assert!(report.failures.is_empty(), "{:?}", report.failures);
            // Crawl exports ignore the outer filter, skip unknown AudioSource,
            // and always append the referenced object's path id.
            assert_eq!(report.written.len(), 1);
            assert_eq!(files.0.len(), 1);
            assert_eq!(
                files.0[b"root/clip_3.wav".as_slice()],
                decoded.as_slice()[0].bytes.as_slice()
            );
        } else {
            assert!(files.0.is_empty());
            assert!(report.written.is_empty());
            assert_eq!(report.failures.len(), 1);
            assert_eq!(report.failures.as_slice()[0].object.path_id(), 3);
            assert!(matches!(
                report.failures.as_slice()[0].error,
                extractor::Error::Audio(unityrs_export::audio::Error::Audio(
                    unityrs_audio::Error::VorbisHeadersRequired
                ))
            ));
        }
    }
    drop((decoded, plan, headers));
    assert_eq!(m.used(), 0);
}
