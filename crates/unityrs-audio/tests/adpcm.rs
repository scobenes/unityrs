mod support;
use std::sync::Arc;
use unityrs_audio::{Error, decode, plan};
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

fn replay(prefix: &str) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/audio.json")).unwrap();
    let mut exact = 0;
    let mut refused = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["case"].as_str().unwrap().starts_with(prefix))
    {
        let label = case["case"].as_str().unwrap();
        let data = hex(case["data"].as_str().unwrap());
        let m = support::meter();
        {
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
                    assert!(
                        case["oracle"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("TOOMANYCHANNELS"),
                        "{label}: {}",
                        case["oracle"]
                    );
                }
                "error" => {
                    assert!(
                        matches!(p, Err(Error::Invalid("ADPCM first block"))),
                        "{label}: {p:?}"
                    );
                    assert_eq!(case["oracle"]["error"], "FmodError", "{label}");
                    assert!(
                        case["oracle"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("FILE BAD"),
                        "{label}"
                    );
                    refused += 1;
                }
                "emptyfirst" => assert!(matches!(p, Err(Error::EmptyFirstSubsound)), "{label}"),
                "ok" => {
                    let p = p.unwrap_or_else(|e| panic!("{label}: {e}"));
                    let files = decode(&p, b"clip", &m).unwrap();
                    assert_eq!(
                        files
                            .as_slice()
                            .iter()
                            .map(|f| f.bytes.len() as u64)
                            .sum::<u64>(),
                        p.output_bytes()
                    );
                    if case["tier_c"] == true {
                        assert!(case["oracle"]["error"].is_string(), "{label}");
                    } else {
                        let expected = case["oracle"]["files"]
                            .as_object()
                            .unwrap_or_else(|| panic!("{label}: {case}"));
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
                    }
                }
                other => panic!("unexpected {label}: {other}"),
            }
        }
        assert_eq!(m.used(), 0, "{label}");
    }
    for channels in [2, 3, 6, 8] {
        for count in [257, 259, 511, 512] {
            for shape in ["full", "short1", "short_frame"] {
                let label = format!("{prefix}c{channels}_count{count}_{shape}");
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
    }
    assert!(exact >= 89, "missing {prefix} oracle coverage: {exact}");
    assert_eq!(refused, 8);
}

#[test]
fn replay_pinned_ima_adpcm_exact() {
    replay("ima_");
}
#[test]
fn replay_pinned_fadpcm_exact() {
    replay("fadpcm_");
}

#[test]
fn fadpcm_short_block_requires_a_complete_output_frame() {
    // FMOD: stereo, 259 samples, 420 bytes; the final three samples of
    // both channels are silent even though channel 0 has all 140 bytes.
    let mut frame = vec![0; 12];
    frame[4..8].fill(0x44);
    frame.extend_from_slice(&[0x11; 128]);
    let payload = frame.repeat(3);
    let data = support::bank(16, &[(&payload, 259, 2, 8)]);
    let m = support::meter();
    let p = plan(&data, true, &m).unwrap();
    let files = decode(&p, b"clip", &m).unwrap();
    let pcm = &files.as_slice()[0].bytes.as_slice()[44..];
    assert!(
        pcm[..256 * 4]
            .as_chunks::<2>()
            .0
            .iter()
            .all(|&s| s == [16, 0])
    );
    assert_eq!(&pcm[256 * 4..], &[0; 12]);
}

#[test]
fn header_sample_counts_bound_output_before_decode() {
    for (mode, frame_size) in [(7, 0x24), (16, 0x8c)] {
        let frame = vec![0; frame_size];
        let data = support::bank(mode, &[(&frame, 1 << 20, 1, 8)]);
        let m = support::meter();
        let p = plan(&data, true, &m).unwrap();
        assert_eq!(p.output_bytes(), 44 + (2 << 20));
        assert!(m.used() < 4096, "plan allocated PCM");
        let small = Meter::new(4096, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(decode(&p, b"clip", &small), Err(Error::Charge(_))));
        assert_eq!(small.used(), 0);
        let huge = support::bank(mode, &[(&frame, 0x3fff_ffff, 8, 8)]);
        assert!(matches!(plan(&huge, true, &m), Err(Error::OutputSize)));
    }
}

#[test]
fn decode_work_is_charged_per_frame_and_refusal_releases_buffers() {
    for (mode, size, samples) in [(7, 0x24, 64), (16, 0x8c, 256)] {
        let data = support::bank(mode, &[(&vec![0; size * 3], samples * 3, 1, 8)]);
        let m = support::meter();
        let p = plan(&data, true, &m).unwrap();
        // count + filename + zero-initialized WAV, then exactly three frames.
        let before = m.decoded();
        drop(decode(&p, b"clip", &m).unwrap());
        assert_eq!(
            m.decoded() - before,
            1 + 8 + p.output_bytes() + u64::from(samples) * 3
        );
        let work = 1 + 8 + p.output_bytes() + u64::from(samples);
        let limited =
            Meter::with_decode_work(1 << 20, work, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            decode(&p, b"clip", &limited),
            Err(Error::Charge(_))
        ));
        assert_eq!(limited.used(), 0);
    }
}

#[test]
fn mutated_banks_never_panic_or_exceed_the_plan_or_leak() {
    let mut rng = unityrs_testkit::Rng::new(0xadfc_5002);
    for (mode, size, samples) in [(7, 0x24, 64), (16, 0x8c, 256)] {
        let mut reached = 0;
        for _ in 0..10_000 {
            let channels = [1, 2, 6, 8][rng.below(4)];
            let mut payload = vec![0; size * usize::from(channels) * 3];
            for b in &mut payload {
                *b = u8::try_from(rng.below(256)).unwrap();
            }
            let mut data = support::bank(mode, &[(&payload, samples * 3, channels, 8)]);
            rng.flip_bits(&mut data, 3);
            if rng.below(2) == 0 {
                rng.truncate(&mut data);
            }
            let m = Meter::with_decode_work(
                64 << 10,
                64 << 10,
                Arc::new(Unbounded),
                Reservation::Input,
            );
            if let Ok(p) = plan(&data, true, &m)
                && let Ok(files) = decode(&p, b"mutated", &m)
            {
                assert_eq!(
                    files
                        .as_slice()
                        .iter()
                        .map(|f| f.bytes.len() as u64)
                        .sum::<u64>(),
                    p.output_bytes()
                );
                reached += 1;
            }
            assert_eq!(m.used(), 0);
        }
        assert!(reached > 100, "codec {mode} did not reach expansion");
    }
}
