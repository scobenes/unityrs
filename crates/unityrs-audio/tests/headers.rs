mod support;
use std::sync::Arc;
use support::{bank, meter};
use unityrs_audio::{Error, decode, plan};
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};

#[test]
fn passthrough_precedes_bank_and_uses_magic_only() {
    let m = meter();
    for (data, suffix) in [
        (b"OggS".as_slice(), b".ogg".as_slice()),
        (b"RIFFftyp", b".wav"),
        (b"1234ftyp", b".m4a"),
    ] {
        let p = plan(data, true, &m).unwrap();
        assert!(p.header().is_none());
        let out = decode(&p, b"sound", &m).unwrap();
        assert_eq!(out.as_slice()[0].bytes.as_slice(), data);
        assert_eq!(
            out.as_slice()[0].name.as_slice(),
            [b"sound", suffix].concat()
        );
    }
    for data in [b"".as_slice(), b"FSB", b"ID3x", b"FORM", b"xxxx"] {
        assert!(matches!(
            plan(data, true, &m),
            Err(Error::UnsupportedContainer)
        ));
    }
}

#[test]
fn names_versions_rates_and_extra_chunks() {
    let m = meter();
    for (i, hz) in [
        4000, 8000, 11000, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 96000,
    ]
    .into_iter()
    .enumerate()
    {
        let data = bank(2, &[(&[1, 2, 3, 4], 2, 1, u8::try_from(i).unwrap())]);
        let p = plan(&data, true, &m).unwrap();
        assert_eq!(p.subsounds()[0].frequency, hz);
    }
    let mut data = bank(2, &[(&[1, 2, 3, 4], 2, 1, 8)]);
    // A channel override, a rate override and an unknown chunk; then name offsets.
    let chunks = [
        ((1u32 << 25) | (1 << 1) | 1).to_le_bytes().as_slice(),
        &[1],
        ((2u32 << 25) | (4 << 1) | 1).to_le_bytes().as_slice(),
        &12345u32.to_le_bytes(),
        ((127u32 << 25) | (3 << 1)).to_le_bytes().as_slice(),
        &[1, 2, 3],
    ]
    .concat();
    data[60] |= 1;
    data.splice(68..68, chunks.iter().copied());
    data[12..16].copy_from_slice(&(8 + u32::try_from(chunks.len()).unwrap()).to_le_bytes());
    let names = [4u32.to_le_bytes().as_slice(), b"bank-name\0"].concat();
    data.splice(68 + chunks.len()..68 + chunks.len(), names.iter().copied());
    data[16..20].copy_from_slice(&(u32::try_from(names.len()).unwrap()).to_le_bytes());
    let p = plan(&data, true, &m).unwrap();
    let sub = &p.subsounds()[0];
    assert_eq!(sub.frequency, 12345);
    assert_eq!(sub.channels, 1);
    assert_eq!(sub.name, Some(b"bank-name".as_slice()));
    assert_eq!(sub.extra_chunks, chunks);
    assert_eq!(
        decode(&p, b"clip", &m).unwrap().as_slice()[0]
            .name
            .as_slice(),
        b"clip.wav"
    );
    // Version 0 adds four bytes to the base header.
    data[4..8].copy_from_slice(&0u32.to_le_bytes());
    data.splice(60..60, [0; 4]);
    assert_eq!(
        plan(&data, true, &m).unwrap().header().unwrap().header_size,
        64
    );
}

#[test]
fn hostile_headers_and_every_truncation_refuse_without_panics() {
    let m = meter();
    let valid = bank(2, &[(&[0; 8], 4, 1, 8), (&[0; 8], 4, 1, 8)]);
    for end in 4..valid.len() {
        assert!(plan(&valid[..end], true, &m).is_err(), "cut {end}");
        assert_eq!(m.used(), 0);
    }
    for (offset, value) in [
        (4, 2),
        (8, u32::MAX),
        (12, u32::MAX),
        (16, u32::MAX),
        (20, u32::MAX),
    ] {
        let mut data = valid.clone();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(plan(&data, true, &m).is_err());
        assert_eq!(m.used(), 0);
    }
    for (offset, word) in [
        (60, 15u64 << 1 | 4 << 34),
        (60, 8 << 1 | 1 << 7 | 4 << 34),
        (68, 8 << 1 | 4 << 34),
        (60, 8 << 1 | 0x3fff_ffff << 34),
        (60, 8 << 1 | 4 << 34 | 1),
    ] {
        let mut data = valid.clone();
        data[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
        assert!(plan(&data, true, &m).is_err(), "word {word:x}");
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn memory_and_work_refusals_release_every_charge() {
    let data = bank(2, &[(&[0; 8], 4, 1, 8)]);
    for memory in [0, 32, 128, 256, 512, 1024, 4096] {
        let m = Meter::new(memory, Arc::new(Unbounded), Reservation::Input);
        if let Ok(p) = plan(&data, true, &m) {
            let _ = decode(&p, b"clip", &m);
        }
        assert_eq!(m.used(), 0);
    }
    let m = Meter::with_decode_work(1 << 20, 1, Arc::new(Unbounded), Reservation::Input);
    assert!(matches!(plan(&data, true, &m), Err(Error::Charge(_))));
    assert_eq!(m.used(), 0);
    let m = meter();
    let p = plan(&data, true, &m).unwrap();
    let limited = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
    assert!(matches!(
        decode(&p, b"clip", &limited),
        Err(Error::Charge(_))
    ));
    assert_eq!(limited.used(), 0);
}

#[test]
fn bad_chunks_names_and_wav_fields_are_typed_errors() {
    let m = meter();
    let base = bank(2, &[(&[0; 8], 4, 1, 8)]);
    for chunks in [
        [(1u32 << 25 | 2).to_le_bytes().as_slice(), &[0]].concat(),
        [
            (2u32 << 25 | 8).to_le_bytes().as_slice(),
            &u32::MAX.to_le_bytes(),
        ]
        .concat(),
        [
            (2u32 << 25 | 8).to_le_bytes().as_slice(),
            &1_500_000_000u32.to_le_bytes(),
        ]
        .concat(),
        [(127u32 << 25 | 0x01ff_fffe).to_le_bytes().as_slice(), &[0]].concat(),
        (127u32 << 25 | 1).to_le_bytes().to_vec(),
    ] {
        let mut data = base.clone();
        data[60] |= 1;
        data.splice(68..68, chunks.iter().copied());
        data[12..16].copy_from_slice(&(8 + u32::try_from(chunks.len()).unwrap()).to_le_bytes());
        assert!(plan(&data, true, &m).is_err());
        assert_eq!(m.used(), 0);
    }
    for names in [
        0u32.to_le_bytes().to_vec(),
        100u32.to_le_bytes().to_vec(),
        [4u32.to_le_bytes().as_slice(), b"no-nul"].concat(),
    ] {
        let mut data = base.clone();
        data.splice(68..68, names.iter().copied());
        data[16..20].copy_from_slice(&u32::try_from(names.len()).unwrap().to_le_bytes());
        assert!(plan(&data, true, &m).is_err());
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn channel_override_replaces_header_bits() {
    let mut data = bank(2, &[(&[0; 32], 2, 2, 8)]);
    data[60] |= 1;
    let chunks = [(1u32 << 25 | 2).to_le_bytes().as_slice(), &[3]].concat();
    data.splice(68..68, chunks.iter().copied());
    data[12..16].copy_from_slice(&13u32.to_le_bytes());
    let m = meter();
    let p = plan(&data, true, &m).unwrap();
    assert_eq!(p.subsounds()[0].channels, 3);
    let files = decode(&p, b"three", &m).unwrap();
    let wav = files.as_slice()[0].bytes.as_slice();
    assert_eq!(&wav[22..24], &3u16.to_le_bytes());
    assert_eq!(wav.len(), 44 + 12);
}

#[test]
fn excessive_channels_refuse_before_any_metadata_allocation() {
    for (mode, size, samples) in [
        (1, 4, 4),
        (2, 8, 4),
        (3, 12, 4),
        (4, 16, 4),
        (5, 16, 4),
        (7, 36, 64),
        (16, 140, 256),
    ] {
        for channels in [33, 64] {
            for index in [0, 1] {
                let payload = vec![0; size * usize::from(channels)];
                let mut data = bank(
                    mode,
                    &[(&payload, samples, 1, 8), (&payload, samples, 1, 8)],
                );
                let at = 60 + index * 8;
                data[at] |= 1;
                let chunk = [(1u32 << 25 | 2).to_le_bytes().as_slice(), &[channels]].concat();
                data.splice(at + 8..at + 8, chunk);
                data[12..16].copy_from_slice(&21u32.to_le_bytes());
                let m = meter();
                let p = plan(&data, true, &m);
                assert!(
                    matches!(p, Err(Error::TooManyChannels { channels: n }) if n == u16::from(channels)),
                    "mode {mode}, channels {channels}: {p:?}"
                );
                assert_eq!(m.granted(), 0, "allocated before channel refusal");
            }
        }
    }
}

#[test]
fn invalid_early_record_does_not_reserve_the_declared_count() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome};

    #[derive(Debug, Default)]
    struct Peak {
        live: AtomicU64,
        peak: AtomicU64,
    }
    impl PermitHome for Peak {
        fn release(&self, bytes: u64) {
            self.live.fetch_sub(bytes, Ordering::Relaxed);
        }
    }
    #[derive(Debug)]
    struct Budget(Arc<Peak>);
    impl MemoryBudget for Budget {
        fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
            let live = self.0.live.fetch_add(bytes, Ordering::Relaxed) + bytes;
            self.0.peak.fetch_max(live, Ordering::Relaxed);
            Ok(Permit::new(bytes, self.0.clone()))
        }
    }
    // The count fits the real 1 MiB section, but its first record is invalid.
    // Checking only count * 8 <= section length used to reserve about 9 MiB.
    for valid_prefix in [false, true] {
        let mut data = bank(2, &[(&[0; 8], 4, 1, 8)]);
        data.resize(60 + (1 << 20), 0);
        data[8..12].copy_from_slice(&(1u32 << 17).to_le_bytes());
        data[12..16].copy_from_slice(&(1u32 << 20).to_le_bytes());
        data[20..24].copy_from_slice(&0u32.to_le_bytes());
        let at = if valid_prefix { 68 } else { 60 };
        data[at..at + 8].copy_from_slice(&(15u64 << 1).to_le_bytes());
        let peak = Arc::new(Peak::default());
        let m = Meter::new(u64::MAX, Arc::new(Budget(peak.clone())), Reservation::Input);
        assert!(matches!(
            plan(&data, true, &m),
            Err(Error::Invalid("frequency"))
        ));
        assert_eq!(m.used(), 0);
        assert_eq!(peak.live.load(Ordering::Relaxed), 0);
        assert!(
            peak.peak.load(Ordering::Relaxed) <= 1024,
            "declared count charged metadata"
        );
    }
}
