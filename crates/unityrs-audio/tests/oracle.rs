mod support;
use unityrs_audio::{Error, decode, extension, plan};

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one recorded case dispatch keeps the fixture context"
)]
fn replay_pinned_audio_exports() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio.json");
    let body = std::fs::read_to_string(path)
        .expect("coordinator: run tools/fixtures/audio.py in the pinned oracle");
    let fixture: serde_json::Value = serde_json::from_str(&body).unwrap();
    let mut exact = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let label = case["case"].as_str().unwrap();
        let data = hex(case["data"].as_str().unwrap());
        let m = support::meter();
        if let Some(want) = case["extension"].as_str() {
            assert_eq!(
                extension(
                    case["compression"]
                        .as_i64()
                        .map(|n| i32::try_from(n).unwrap())
                ),
                want,
                "{label}"
            );
        }
        let p = plan(&data, true, &m);
        match case["rust"].as_str().unwrap() {
            "channels" => {
                assert!(
                    matches!(p, Err(Error::TooManyChannels { channels })
                    if u64::from(channels) == case["channels"].as_u64().unwrap()),
                    "{label}: {p:?}"
                );
                assert_eq!(m.granted(), 0, "{label}");
                assert_eq!(case["oracle"]["error"], "FmodError", "{label}");
                let error = if label.starts_with("refused_pcm") {
                    "VERSION"
                } else {
                    "TOOMANYCHANNELS"
                };
                assert_eq!(
                    case["oracle"]["message"], error,
                    "{label}: {}",
                    case["oracle"]
                );
            }
            "unsupported" => assert!(
                matches!(
                    p,
                    Err(Error::Unsupported { .. }
                        | Error::UnsupportedContainer
                        | Error::Invalid("VORBISDATA absent"))
                ),
                "{label}: {p:?}"
            ),
            "emptyfirst" => assert!(matches!(p, Err(Error::EmptyFirstSubsound)), "{label}"),
            "error" => assert!(p.is_err(), "{label}"),
            "ok" => {
                let p = p.unwrap_or_else(|e| panic!("{label}: {e}"));
                let files = decode(&p, case["name"].as_str().unwrap().as_bytes(), &m).unwrap();
                if let Some(expected) = case["oracle"]["files"].as_object() {
                    assert_eq!(files.len(), expected.len(), "{label}");
                    for file in files.as_slice() {
                        let name = std::str::from_utf8(file.name.as_slice()).unwrap();
                        assert_eq!(
                            file.bytes.as_slice(),
                            hex(expected[name].as_str().unwrap()),
                            "{label}: {name}"
                        );
                    }
                    exact += 1;
                } else {
                    assert!(
                        case["tier_c"] == true
                            || label.starts_with("refused_pcm")
                            || label == "version0",
                        "unexplained oracle refusal: {case}"
                    );
                }
            }
            other => panic!("unknown replay expectation {other}"),
        }
        assert_eq!(m.used(), 0, "{label}");
    }
    for codec in [
        "refused_pcm1",
        "pcm16",
        "refused_pcm3",
        "refused_pcm4",
        "refused_pcm5",
        "ima",
        "fadpcm",
    ] {
        for channels in [32, 33, 64] {
            let label = format!("{codec}_c{channels}_limit");
            assert!(
                fixture["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["case"] == label),
                "missing {label}"
            );
        }
    }
    // Do not let the broad replay silently lose the PCM16 reference cases.
    for channels in [1, 2] {
        for rate in 0..11 {
            let label = format!("pcm16_c{channels}_r{rate}");
            assert!(
                fixture["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|case| { case["case"] == label && case["oracle"]["files"].is_object() }),
                "missing accepted PCM16 oracle case {label}"
            );
        }
    }
    assert!(exact >= 25, "oracle accepted too few cases: {exact}");
}

#[test]
fn replay_pinned_frequency_overrides_float32_round_trip() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/audio.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    for frequency in [44100, 16_777_215, 16_777_216, 16_777_217, 16_777_219] {
        let label = format!("frequency_override_{frequency}");
        let case = cases.iter().find(|case| case["case"] == label).expect(
            "coordinator: regenerate audio.json in the pinned oracle: python3 tools/fixtures/audio.py",
        );
        let expected = case["oracle"]["files"]
            .as_object()
            .expect("frequency override must be accepted by the pinned oracle");
        assert_eq!(expected.len(), 1, "{label}");
        let data = hex(case["data"].as_str().unwrap());
        let m = support::meter();
        {
            let p = plan(&data, true, &m).unwrap();
            let files = decode(&p, case["name"].as_str().unwrap().as_bytes(), &m).unwrap();
            assert_eq!(files.len(), expected.len(), "{label}");
            for file in files.as_slice() {
                let name = std::str::from_utf8(file.name.as_slice()).unwrap();
                // The full oracle WAV pins frequency, byte rate and PCM bytes.
                assert_eq!(
                    file.bytes.as_slice(),
                    hex(expected[name].as_str().unwrap()),
                    "{label}"
                );
            }
        }
        assert_eq!(m.used(), 0, "{label}");
    }
}

#[test]
fn replay_pinned_extension_property_including_enum_collisions() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/audio.json")).unwrap();
    let cases = fixture["extensions"]
        .as_array()
        .expect("coordinator: regenerate audio.json with python3 tools/fixtures/audio.py");
    let mut keys = std::collections::BTreeSet::new();
    let mut collisions = [0; 2];
    for case in cases {
        let value = case["compression"]
            .as_i64()
            .map(|n| i32::try_from(n).unwrap());
        assert_eq!(
            extension(value),
            case["extension"].as_str().unwrap(),
            "{}",
            case["case"]
        );
        keys.insert(value);
        if let Some(n @ 1..=2) = value {
            collisions[usize::try_from(n - 1).unwrap()] += 1;
        }
    }
    // Coverage checks keys only; expected extensions come exclusively from the oracle.
    assert_eq!(
        keys,
        [None, Some(-1), Some(999)]
            .into_iter()
            .chain(
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 17, 20, 21, 22, 23, 24
                ]
                .map(Some)
            )
            .collect()
    );
    assert_eq!(collisions, [2, 2]);
    // P3b consumes the integer values of both enums through this property;
    // construction of Python enum objects is covered separately in P3c.
    for (prefix, count) in [("AudioType.", 13), ("AudioCompressionFormat.", 10)] {
        let values: std::collections::BTreeSet<_> = cases
            .iter()
            .filter(|case| case["case"].as_str().unwrap().starts_with(prefix))
            .map(|case| case["compression"].as_i64().unwrap())
            .collect();
        assert_eq!(values.len(), count, "{prefix}");
    }
}
