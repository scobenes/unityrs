mod support;
use unityrs_audio::{Error, VorbisHeaders, decode, decode_with_vorbis, plan};

fn table() -> Vec<u8> {
    // An empty provider contains no FMOD-derived data.
    b"VST1\0\0\0\0".to_vec()
}
fn bank(payload: &[u8], samples: u32) -> Vec<u8> {
    let mut b = support::bank(15, &[(payload, samples, 1, 8)]);
    b[12..16].copy_from_slice(&16u32.to_le_bytes());
    b[60] |= 1;
    b.splice(
        68..68,
        ((11u32 << 25) | 8)
            .to_le_bytes()
            .into_iter()
            .chain(42u32.to_le_bytes()),
    );
    b
}
#[test]
fn vorbis_requires_explicit_runtime_headers_and_bounds_packets() {
    let m = support::meter();
    let b = bank(&[1, 0, 0, 0, 0], 8);
    let p = plan(&b, true, &m).unwrap();
    assert_eq!(p.output_bytes(), 60);
    assert!(matches!(
        decode(&p, b"c", &m),
        Err(Error::VorbisHeadersRequired)
    ));
    let headers = VorbisHeaders::from_bytes(&table(), &m).unwrap();
    #[cfg(feature = "vorbis")]
    assert!(matches!(
        decode_with_vorbis(&p, b"c", Some(&headers), &m),
        Err(Error::VorbisSetupMissing { crc: 42 })
    ));
    #[cfg(not(feature = "vorbis"))]
    assert!(matches!(
        decode_with_vorbis(&p, b"c", Some(&headers), &m),
        Err(Error::Unsupported { codec: 15 })
    ));
    for payload in [&[4, 0, 1][..], &[1][..]] {
        assert!(plan(&bank(payload, 8), true, &m).is_err());
    }
    assert!(matches!(
        plan(&bank(&[0, 0], 1), true, &m),
        Err(Error::VorbisShortSubsound)
    ));
    drop(headers);
    drop(p);
    assert_eq!(m.used(), 0);
}
#[test]
fn runtime_table_is_bounded_and_path_loaded() {
    let m = support::meter();
    for bytes in [b"".as_slice(), b"VST1", b"VST1\x01\0\0\0", b"VST1\0\0\0\0x"] {
        assert!(VorbisHeaders::from_bytes(bytes, &m).is_err());
        assert_eq!(m.used(), 0);
    }
    let dir = unityrs_testkit::scratch::Scratch::new("vorbis-table");
    let path = dir.path().join("headers");
    std::fs::write(&path, table()).unwrap();
    let headers = VorbisHeaders::load(&path, &m).unwrap();
    drop(headers);
    assert_eq!(m.used(), 0);
    assert!(matches!(
        VorbisHeaders::load(&dir.path().join("absent"), &m),
        Err(Error::VorbisHeadersIo(_))
    ));
}

#[path = "support/tol.rs"]
mod tol;
#[cfg(feature = "vorbis")]
fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[cfg(feature = "vorbis")]
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/vorbis.json")).unwrap()
}
#[cfg(feature = "vorbis")]
fn own_headers(case: &serde_json::Value) -> Vec<u8> {
    // This setup packet comes from our independent encoder, not FMOD's table.
    let setup = hex(case["setup"].as_str().unwrap());
    let mut bytes = b"VST1".to_vec();
    for n in [
        1u32,
        u32::try_from(case["crc"].as_u64().unwrap()).unwrap(),
        u32::try_from(setup.len()).unwrap(),
    ] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    bytes.extend(setup);
    bytes
}
#[test]
#[cfg(feature = "vorbis")]
fn independent_encoder_rebuild_decode_and_fmod_refusal() {
    let fixture = fixture();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 12);
    // Literal observed mappings also catch a generator that chooses stream
    // order (or generic WAV speaker order) instead of FMOD's actual order.
    for channels in 1..=32 {
        let expected = match channels {
            6 => vec![0, 2, 1, 5, 3, 4],
            8 => vec![0, 2, 1, 7, 5, 6, 3, 4],
            _ => (0..channels).collect(),
        };
        assert_eq!(
            fixture["ogg_fmod_channel_order"][channels.to_string()],
            serde_json::json!(expected)
        );
    }
    for case in fixture["cases"].as_array().unwrap() {
        let m = support::meter();
        let b = hex(case["bank"].as_str().unwrap());
        let h = VorbisHeaders::from_bytes(&own_headers(case), &m).unwrap();
        let p = plan(&b, true, &m).unwrap();
        let files = decode_with_vorbis(&p, b"tone", Some(&h), &m).unwrap();
        assert_eq!(files.len(), 1);
        let wav = files.as_slice()[0].bytes.as_slice();
        let channels = usize::try_from(case["channels"].as_u64().unwrap()).unwrap();
        assert_eq!(wav.len() as u64, p.output_bytes());
        assert_eq!(&wav[20..22], &1u16.to_le_bytes());
        assert_eq!(&wav[34..36], &16u16.to_le_bytes());
        let metrics = tol::compare(
            &hex(case["independent_fmod_order_pcm16"].as_str().unwrap()),
            &wav[44..],
            channels,
            90.0,
            2,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", case["case"]));
        eprintln!("{}: {metrics:?}", case["case"]);
        for metric in metrics {
            assert!(metric.snr_db >= 90.0 && metric.max_lsb <= 2);
        }
        // This replays the recorded refusal; it does not verify FMOD PCM parity.
        assert_eq!(case["oracle"]["error"], "FmodError");
        assert_eq!(case["oracle"]["message"], "VERSION");
        drop(files);
        drop(p);
        drop(h);
        assert_eq!(m.used(), 0);
    }
}
#[test]
fn tolerance_checks_each_channel_count_silence_and_both_thresholds() {
    let pcm = |v: &[i16]| v.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
    let r = pcm(&[30000, 0, 30000, 0]);
    let exact = tol::compare(&r, &r, 2, 90.0, 2).unwrap();
    assert!(
        exact
            .iter()
            .all(|m| m.snr_db.is_infinite() && m.max_lsb == 0)
    );
    assert!(tol::compare(&r, &r[..4], 2, 90.0, 2).is_err());
    assert!(tol::compare(&r, &pcm(&[30000, 1, 30000, 1]), 2, 90.0, 2).is_err());
    assert!(tol::compare(&pcm(&[10, 10]), &pcm(&[9, 9]), 1, 90.0, 2).is_err());
    assert!(tol::compare(&pcm(&[30000; 100]), &pcm(&[29997; 100]), 1, 0.0, 2).is_err());
}

#[test]
#[cfg(feature = "vorbis")]
fn malformed_setup_packets_and_mutations_never_panic_or_leak() {
    let fixture = fixture();
    let case = &fixture["cases"][0];
    let b = hex(case["bank"].as_str().unwrap());
    let valid = own_headers(case);
    let m = support::meter();
    for end in 0..valid.len() {
        let _ = VorbisHeaders::from_bytes(&valid[..end], &m);
        assert_eq!(m.used(), 0);
    }
    // A well-framed but invalid setup is refused by the contained decoder.
    let mut bad = valid.clone();
    bad[23..].fill(255);
    {
        let h = VorbisHeaders::from_bytes(&bad, &m).unwrap();
        let p = plan(&b, true, &m).unwrap();
        assert!(decode_with_vorbis(&p, b"bad", Some(&h), &m).is_err());
    }
    assert_eq!(m.used(), 0);
    let h = VorbisHeaders::from_bytes(&valid, &m).unwrap();
    let baseline = m.used();
    for end in 0..b.len() {
        if let Ok(p) = plan(&b[..end], true, &m) {
            let _ = decode_with_vorbis(&p, b"bad", Some(&h), &m);
        }
        assert_eq!(m.used(), baseline);
    }
    for at in (0..b.len()).step_by(7) {
        let mut mutated = b.clone();
        mutated[at] ^= 255;
        if let Ok(p) = plan(&mutated, true, &m) {
            let _ = decode_with_vorbis(&p, b"bad", Some(&h), &m);
        }
        assert_eq!(m.used(), baseline);
    }
}

#[test]
#[cfg(all(feature = "vorbis", target_os = "linux"))]
fn runtime_oracle_table_stays_in_memory() {
    let Some(path) = std::env::var_os("UNITYRS_VORBIS_HEADERS_JSON") else {
        eprintln!("runtime table test requires UNITYRS_VORBIS_HEADERS_JSON in the oracle image");
        return;
    };
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/fixtures/audio_vorbis.py");
    let output = unityrs_testkit::child::output(
        std::process::Command::new("python")
            .arg(script)
            .arg("--runtime-table")
            .arg(path),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let m = support::meter();
    let h = VorbisHeaders::from_bytes(&output.stdout, &m).unwrap();
    let mut rest = &output.stdout[8..];
    let mut decoded = 0;
    // All IDs/setup bytes here come from the image at test time. No derived
    // bank, PCM, digest or table is written anywhere, including scratch.
    while !rest.is_empty() {
        let crc = u32::from_le_bytes(rest[..4].try_into().unwrap());
        let size = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
        rest = &rest[8 + size..];
        for channels in [1u8, 2] {
            let mut b = bank(&[1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0], 256);
            b[72..76].copy_from_slice(&crc.to_le_bytes());
            b[60] |= u8::from(channels == 2) << 5;
            let p = plan(&b, true, &m).unwrap();
            if let Ok(files) = decode_with_vorbis(&p, b"runtime", Some(&h), &m) {
                assert!(
                    files.as_slice()[0].bytes.as_slice()[44..]
                        .iter()
                        .all(|&b| b == 0)
                );
                decoded += 1;
                break;
            }
        }
    }
    assert!(
        decoded > 0,
        "no runtime-table header could decode independent silence packets"
    );
    drop(h);
    assert_eq!(m.used(), 0);
}

#[test]
#[cfg(feature = "vorbis")]
fn editor_fixtures_use_the_same_per_channel_fmod_tolerance() {
    let Some(path) = std::env::var_os("UNITYRS_AUDIO_EDITOR_MANIFEST") else {
        eprintln!("G8 Editor fixtures pending; set UNITYRS_AUDIO_EDITOR_MANIFEST to replay them");
        return;
    };
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = manifest.as_array().unwrap();
    assert_ne!(cases.len(), 0);
    for case in cases {
        let m = support::meter();
        let b = std::fs::read(case["bank"].as_str().unwrap()).unwrap();
        let h =
            VorbisHeaders::load(std::path::Path::new(case["table"].as_str().unwrap()), &m).unwrap();
        let p = plan(&b, true, &m).unwrap();
        let files = decode_with_vorbis(&p, case["name"].as_str().unwrap().as_bytes(), Some(&h), &m)
            .unwrap();
        let expected = case["files"].as_object().unwrap();
        assert_eq!(files.len(), expected.len());
        for file in files.as_slice() {
            let name = std::str::from_utf8(file.name.as_slice()).unwrap();
            let oracle = std::fs::read(expected[name].as_str().unwrap()).unwrap();
            let actual = file.bytes.as_slice();
            assert_eq!(actual[..44], oracle[..44], "{name}: WAV metadata/count");
            let channels = usize::from(u16::from_le_bytes(actual[22..24].try_into().unwrap()));
            tol::compare(&oracle[44..], &actual[44..], channels, 90.0, 2).unwrap();
        }
    }
}

#[test]
#[cfg(feature = "vorbis")]
fn wav_reservation_fails_before_entering_the_decoder() {
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        limits::Meter,
    };
    let case = &fixture()["cases"][1];
    let data = hex(case["bank"].as_str().unwrap());
    let provider_meter = support::meter();
    let mut bytes = own_headers(case);
    bytes[23..].fill(255); // Valid table framing, deliberately invalid codebooks.
    let headers = VorbisHeaders::from_bytes(&bytes, &provider_meter).unwrap();
    let m = Meter::new(1024, Arc::new(Unbounded), Reservation::Input);
    let p = plan(&data, true, &m).unwrap();
    let baseline = m.used();
    assert!(matches!(
        decode_with_vorbis(&p, b"c", Some(&headers), &m),
        Err(Error::Charge(_))
    ));
    assert_eq!(m.used(), baseline);
    drop(p);
    assert_eq!(m.used(), 0);
}

#[test]
#[cfg(feature = "vorbis")]
fn multiple_subsounds_sentinels_trimming_and_insufficient_packets() {
    let case = &fixture()["cases"][0];
    let single = hex(case["bank"].as_str().unwrap());
    let m = support::meter();
    let headers = VorbisHeaders::from_bytes(&own_headers(case), &m).unwrap();
    let payload = &single[76..];
    let padded = payload.len().next_multiple_of(32);
    let mut bank = single[..60].to_vec();
    bank[8..12].copy_from_slice(&2u32.to_le_bytes());
    bank[12..16].copy_from_slice(&32u32.to_le_bytes());
    bank[20..24].copy_from_slice(&u32::try_from(padded + payload.len()).unwrap().to_le_bytes());
    bank.extend_from_slice(&single[60..76]);
    let mut second = single[60..76].to_vec();
    let word = u64::from_le_bytes(second[..8].try_into().unwrap()) | (padded as u64 / 32) << 7;
    second[..8].copy_from_slice(&word.to_le_bytes());
    bank.extend(second);
    bank.extend_from_slice(payload);
    bank.resize(92 + padded, 0);
    bank.extend_from_slice(payload);
    let p = plan(&bank, true, &m).unwrap();
    let files = decode_with_vorbis(&p, b"multi", Some(&headers), &m).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files.as_slice()[0].name.as_slice(), b"multi.wav");
    assert_eq!(files.as_slice()[1].name.as_slice(), b"multi-1.wav");
    assert_eq!(
        files.as_slice()[0].bytes.as_slice(),
        files.as_slice()[1].bytes.as_slice()
    );
    for count in [2u64, 100, 10000] {
        for eos in [0u16, u16::MAX] {
            let mut b = single.clone();
            let word =
                (u64::from_le_bytes(b[60..68].try_into().unwrap()) & ((1 << 34) - 1)) | count << 34;
            b[60..68].copy_from_slice(&word.to_le_bytes());
            let end = b.len();
            b[end - 2..].copy_from_slice(&eos.to_le_bytes());
            let p = plan(&b, true, &m).unwrap();
            let got = decode_with_vorbis(&p, b"trim", Some(&headers), &m);
            if count == 10000 {
                assert!(matches!(
                    got,
                    Err(Error::Invalid("Vorbis sample count exceeds packets"))
                ));
            } else {
                let got = got.unwrap();
                let pcm = &got.as_slice()[0].bytes.as_slice()[44..];
                assert_eq!(
                    pcm,
                    &files.as_slice()[0].bytes.as_slice()
                        [44..44 + usize::try_from(count).unwrap() * 2]
                );
            }
        }
    }
}
