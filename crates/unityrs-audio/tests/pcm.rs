mod support;
use support::{bank, meter};
use unityrs_audio::{Error, decode, plan};

#[test]
fn frequency_round_trip_precedes_wav_fields_and_byte_rate_validation() {
    for (frequency, expected) in [
        (44100u32, 44100u32),
        (16_777_215, 16_777_215),
        (16_777_216, 16_777_216),
        (16_777_217, 16_777_216),
        (16_777_219, 16_777_220),
        // Rounding reaches 2^30, whose PCM16 byte rate exceeds i32::MAX.
        (1_073_741_823, 1_073_741_824),
    ] {
        // One mono PCM16 subsound, two frames, with a frequency chunk.
        let mut data = [0u8; 80];
        data[..4].copy_from_slice(b"FSB5");
        for (offset, value) in [(4, 1u32), (8, 1), (12, 16), (20, 4), (24, 2)] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        data[60..68].copy_from_slice(&((2u64 << 34) | (8 << 1) | 1).to_le_bytes());
        data[68..72].copy_from_slice(&((2u32 << 25) | (4 << 1)).to_le_bytes());
        data[72..76].copy_from_slice(&frequency.to_le_bytes());
        let m = meter();
        if expected == 1_073_741_824 {
            assert!(matches!(plan(&data, true, &m), Err(Error::OutputSize)));
        } else {
            let p = plan(&data, true, &m).unwrap();
            assert_eq!(p.subsounds()[0].frequency, expected, "{frequency}");
            let files = decode(&p, b"clip", &m).unwrap();
            let wav = files.as_slice()[0].bytes.as_slice();
            assert_eq!(&wav[24..28], &expected.to_le_bytes(), "{frequency}");
            assert_eq!(&wav[28..32], &(expected * 2).to_le_bytes(), "{frequency}");
        }
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn pcm16_wav_and_numbered_outputs() {
    let data = bank(
        2,
        &[(&[0, 128, 255, 127], 2, 1, 8), (&[1, 2, 3, 4], 1, 2, 9)],
    );
    let m = meter();
    let p = plan(&data, true, &m).unwrap();
    assert_eq!(p.output_bytes(), 96);
    let files = decode(&p, b"clip", &m).unwrap();
    assert_eq!(files.as_slice()[0].name.as_slice(), b"clip.wav");
    assert_eq!(files.as_slice()[1].name.as_slice(), b"clip-1.wav");
    let wav = files.as_slice()[1].bytes.as_slice();
    assert_eq!(wav.len(), 48);
    assert_eq!(&wav[..12], b"RIFF(\0\0\0WAVE");
    assert_eq!(&wav[22..24], &2u16.to_le_bytes());
    assert_eq!(&wav[24..28], &48000u32.to_le_bytes());
    assert_eq!(&wav[44..], &[1, 2, 3, 4]);
    drop(files);
    drop(p);
    assert_eq!(m.used(), 0);
}

#[test]
fn widths_and_numpy_float_rule() {
    for (mode, bits) in [(1, 8), (2, 16), (3, 24), (4, 32)] {
        let bytes = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let data = bank(mode, &[(&bytes, 12 / (bits / 8), 1, 1)]);
        let m = meter();
        let p = plan(&data, true, &m).unwrap();
        let files = decode(&p, b"x", &m).unwrap();
        let wav = files.as_slice()[0].bytes.as_slice();
        assert_eq!(&wav[34..36], &(u16::try_from(bits).unwrap()).to_le_bytes());
        assert_eq!(&wav[44..], &bytes);
    }
    let values = [
        -2.0f32,
        -1.0,
        -0.5,
        -0.00002,
        0.0,
        0.00002,
        0.5,
        1.0,
        2.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let payload: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    let data = bank(5, &[(&payload, u32::try_from(values.len()).unwrap(), 1, 8)]);
    let m = meter();
    let p = plan(&data, true, &m).unwrap();
    let files = decode(&p, b"f", &m).unwrap();
    let expected: Vec<u8> = [
        -32768i16, -32768, -16384, 0, 0, 0, 16384, 32767, 32767, 0, 32767, -32768,
    ]
    .iter()
    .flat_map(|x| x.to_le_bytes())
    .collect();
    assert_eq!(&files.as_slice()[0].bytes.as_slice()[44..], expected);
    let p = plan(&data, false, &m).unwrap();
    let files = decode(&p, b"f", &m).unwrap();
    assert_eq!(
        &files.as_slice()[0].bytes.as_slice()[20..22],
        &3u16.to_le_bytes()
    );
    assert_eq!(&files.as_slice()[0].bytes.as_slice()[44..], payload);
}

#[test]
fn short_pcm_is_tier_c_and_first_empty_is_unsupported() {
    let m = meter();
    let data = bank(2, &[(&[1, 2], 1, 1, 8), (&[], 0, 1, 8)]);
    let p = plan(&data, true, &m).unwrap();
    let files = decode(&p, b"x", &m).unwrap();
    assert_eq!(files.as_slice()[0].bytes.len(), 46);
    assert_eq!(files.as_slice()[1].bytes.len(), 44);
    let data = bank(2, &[(&[], 0, 1, 8)]);
    assert!(matches!(
        plan(&data, true, &m),
        Err(Error::EmptyFirstSubsound)
    ));
    for mode in [7, 11, 15, 16, 17, 99] {
        let data = bank(mode, &[(&[0; 32], 4, 1, 8)]);
        assert!(
            matches!(plan(&data, true, &m), Err(Error::Unsupported { codec }) if codec == mode)
        );
    }
}

#[test]
fn short_widths_and_surround_channel_bits() {
    let m = meter();
    for (mode, payload, size) in [
        (1, vec![128], 45),
        (2, vec![0, 1], 46),
        (3, vec![0, 1, 2], 47),
        (4, vec![0, 1, 2, 3], 48),
        (5, 0.5f32.to_le_bytes().to_vec(), 46),
    ] {
        let data = bank(mode, &[(&payload, 1, 1, 8)]);
        let p = plan(&data, true, &m).unwrap();
        let out = decode(&p, b"short\xff", &m).unwrap();
        assert_eq!(out.as_slice()[0].bytes.len(), size);
        assert_eq!(out.as_slice()[0].name.as_slice(), b"short\xff.wav");
    }
    for channels in [6, 8] {
        let data = bank(2, &[(&[0; 32], 1, channels, 8)]);
        let p = plan(&data, true, &m).unwrap();
        assert_eq!(p.subsounds()[0].channels, u16::from(channels));
        let out = decode(&p, b"surround", &m).unwrap();
        assert_eq!(
            out.as_slice()[0].bytes.len(),
            44 + 2 * usize::from(channels)
        );
    }
}
