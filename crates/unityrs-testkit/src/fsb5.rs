//! Synthetic FSB5 banks and hostile inputs, shared by fuzz seeds and tests.
//! Format facts only: 60/64-byte headers, 32-byte data offsets, packed records.
use std::io;

type Entry<'a> = (&'a [u8], u32, u8, u8, &'a [(u32, &'a [u8])]);

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    out.try_reserve(bytes.len()).map_err(io::Error::other)?;
    out.extend_from_slice(bytes);
    Ok(())
}
fn word(n: usize) -> io::Result<[u8; 4]> {
    Ok(u32::try_from(n).map_err(io::Error::other)?.to_le_bytes())
}

/// Build a bank, with optional names and extra metadata chunks.
pub fn bank(
    mode: u32,
    entries: &[Entry<'_>],
    version: u32,
    names: Option<&[&[u8]]>,
) -> io::Result<Vec<u8>> {
    let (mut headers, mut data, mut table) = (Vec::new(), Vec::new(), Vec::new());
    for &(payload, frames, channels, rate, chunks) in entries {
        let padding = (32 - data.len() % 32) % 32;
        append(&mut data, &[0; 32][..padding])?;
        let channels = match channels {
            1 => 0u64,
            2 => 1,
            6 => 2,
            8 => 3,
            _ => return Err(io::Error::other("FSB5 builder channels")),
        };
        let offset = u64::try_from(data.len() / 32).map_err(io::Error::other)?;
        if offset > 0x07ff_ffff || rate > 15 || frames > 0x3fff_ffff {
            return Err(io::Error::other("FSB5 builder record overflow"));
        }
        let raw = u64::from(!chunks.is_empty())
            | u64::from(rate) << 1
            | channels << 5
            | offset << 7
            | u64::from(frames) << 34;
        append(&mut headers, &raw.to_le_bytes())?;
        for (i, &(kind, value)) in chunks.iter().enumerate() {
            let size = u32::try_from(value.len()).map_err(io::Error::other)?;
            if kind > 127 || size > 0x00ff_ffff {
                return Err(io::Error::other("FSB5 builder chunk overflow"));
            }
            let raw = kind << 25 | size << 1 | u32::from(i + 1 < chunks.len());
            append(&mut headers, &raw.to_le_bytes())?;
            append(&mut headers, value)?;
        }
        append(&mut data, payload)?;
    }
    if let Some(names) = names {
        let mut strings = Vec::new();
        let offsets = names
            .len()
            .checked_mul(4)
            .ok_or_else(|| io::Error::other("name offsets overflow"))?;
        for name in names {
            let at = offsets
                .checked_add(strings.len())
                .ok_or_else(|| io::Error::other("name offset overflow"))?;
            append(&mut table, &word(at)?)?;
            append(&mut strings, name)?;
            append(&mut strings, &[0])?;
        }
        append(&mut table, &strings)?;
    }
    let header_size = match version {
        0 => 64,
        1 => 60,
        _ => return Err(io::Error::other("FSB5 builder version")),
    };
    let mut out = Vec::new();
    append(&mut out, b"FSB5")?;
    for n in [
        version.to_le_bytes(),
        word(entries.len())?,
        word(headers.len())?,
        word(table.len())?,
        word(data.len())?,
        mode.to_le_bytes(),
    ] {
        append(&mut out, &n)?;
    }
    let padding = header_size - out.len();
    append(&mut out, &[0; 64][..padding])?;
    append(&mut out, &headers)?;
    append(&mut out, &table)?;
    append(&mut out, &data)?;
    Ok(out)
}

/// Visit PCM/ADPCM banks of both versions, passthroughs and hostile inputs.
pub fn seeds(mut add: impl FnMut(&str, &[u8]) -> io::Result<()>) -> io::Result<()> {
    for (name, data) in [
        ("ogg", b"OggSsynthetic".as_slice()),
        ("riff", b"RIFFsynthetic"),
        ("m4a", b"\0\0\0\x10ftypM4A synthetic"),
        ("riff_before_m4a", b"RIFFftyp"),
        ("legacy_ogg", b"OggSlegacy"),
        ("legacy_non_passthrough", b"FORMsynthetic"),
    ] {
        add(name, data)?;
    }
    let pcm = [
        0, 128, 0, 192, 255, 255, 0, 0, 1, 0, 210, 4, 0, 64, 255, 127,
    ];
    pcm_rates(&mut add, &pcm)?;
    add("version0", &bank(2, &[(&pcm, 8, 1, 8, &[])], 0, None)?)?;
    add(
        "named_multi",
        &bank(
            2,
            &[(&pcm, 8, 1, 8, &[]), (&pcm[..8], 2, 2, 9, &[])],
            1,
            Some(&[b"first", b"second"]),
        )?,
    )?;
    add(
        "overrides",
        &bank(
            2,
            &[(&pcm, 8, 1, 0, &[(1, &[1]), (2, &12345u32.to_le_bytes())])],
            1,
            None,
        )?,
    )?;
    add(
        "short_mono",
        &bank(2, &[(&pcm[..2], 1, 1, 8, &[])], 1, None)?,
    )?;
    add(
        "later_empty",
        &bank(2, &[(&pcm, 8, 1, 8, &[]), (&[], 0, 1, 8, &[])], 1, None)?,
    )?;
    add(
        "empty_first",
        &bank(2, &[(&[], 0, 1, 8, &[]), (&pcm, 8, 1, 8, &[])], 1, None)?,
    )?;
    for (mode, width, name, v0) in [
        (1, 1, "refused_pcm1", "pcm1_v0"),
        (3, 3, "refused_pcm3", "pcm3_v0"),
        (4, 4, "refused_pcm4", "pcm4_v0"),
        (5, 4, "refused_pcm5", "pcm5_v0"),
    ] {
        let entry = (&[0; 24][..], 24 / width, 1, 8, &[][..]);
        add(name, &bank(mode, &[entry], 1, None)?)?;
        add(v0, &bank(mode, &[entry], 0, None)?)?;
    }
    for (mode, name) in [
        (11, "deferred_codec11"),
        (15, "deferred_codec15"),
        (17, "deferred_codec17"),
    ] {
        add(name, &bank(mode, &[(&[0; 64], 32, 1, 8, &[])], 1, None)?)?;
    }
    adpcm_seeds(&mut add)?;
    let good = bank(2, &[(&pcm, 8, 1, 8, &[])], 1, None)?;
    for (name, at, value) in [
        ("hostile_count", 8, u32::MAX),
        ("hostile_headers", 12, u32::MAX),
        ("hostile_names", 16, u32::MAX),
        ("hostile_data", 20, u32::MAX),
        ("hostile_version", 4, 9),
    ] {
        let mut hostile = Vec::new();
        append(&mut hostile, &good)?;
        hostile[at..at + 4].copy_from_slice(&value.to_le_bytes());
        add(name, &hostile)?;
    }
    add("truncated", &good[..good.len() - 1])?;
    let bad_name = bank(2, &[(&pcm, 8, 1, 8, &[])], 1, Some(&[b"broken"]))?;
    add("bad_name", &bad_name[..bad_name.len() - 17])
}

fn adpcm_seeds(add: &mut impl FnMut(&str, &[u8]) -> io::Result<()>) -> io::Result<()> {
    for (mode, size, samples, labels) in [
        (
            7,
            0x24,
            64,
            [
                "ima_mono",
                "ima_stereo",
                "ima_6ch",
                "ima_8ch",
                "ima_3ch",
                "ima_short_last",
                "ima_short_first",
                "ima_trailing",
                "ima_next",
                "ima_large",
            ],
        ),
        (
            16,
            0x8c,
            256,
            [
                "fadpcm_mono",
                "fadpcm_stereo",
                "fadpcm_6ch",
                "fadpcm_8ch",
                "fadpcm_3ch",
                "fadpcm_short_last",
                "fadpcm_short_first",
                "fadpcm_trailing",
                "fadpcm_next",
                "fadpcm_large",
            ],
        ),
    ] {
        let mut payload = [0x12; 0x8c * 8 * 3];
        if mode == 16 {
            for (i, frame) in payload.as_chunks_mut::<0x8c>().0.iter_mut().enumerate() {
                // Coefficient and shift nibbles span all sixteen classes.
                let patterns = [
                    [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
                    [0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76],
                ];
                frame[..8].copy_from_slice(&patterns[i % 2]);
            }
        }
        for (channel, label, old_label) in [
            (
                1,
                labels[0],
                if mode == 7 {
                    "ima_mono_v0"
                } else {
                    "fadpcm_mono_v0"
                },
            ),
            (
                2,
                labels[1],
                if mode == 7 {
                    "ima_stereo_v0"
                } else {
                    "fadpcm_stereo_v0"
                },
            ),
            (
                6,
                labels[2],
                if mode == 7 {
                    "ima_6ch_v0"
                } else {
                    "fadpcm_6ch_v0"
                },
            ),
            (
                8,
                labels[3],
                if mode == 7 {
                    "ima_8ch_v0"
                } else {
                    "fadpcm_8ch_v0"
                },
            ),
        ] {
            let entry = (
                &payload[..size * usize::from(channel) * 3],
                samples * 3,
                channel,
                8,
                &[][..],
            );
            add(label, &bank(mode, &[entry], 1, None)?)?;
            add(old_label, &bank(mode, &[entry], 0, None)?)?;
        }
        adpcm_edges(add, mode, size, samples, labels, &payload)?;
    }
    ima_bad_steps(add)
}

fn ima_bad_steps(add: &mut impl FnMut(&str, &[u8]) -> io::Result<()>) -> io::Result<()> {
    for (channels, step_offset, label) in [
        (1, 2, "ima_bad_mono"),
        (2, 6, "ima_bad_stereo"),
        (6, 18, "ima_bad_6ch"),
    ] {
        let mut payload = [0x12; 0x24 * 6 * 3];
        payload[0x24 * usize::from(channels) + step_offset] = 255;
        add(
            label,
            &bank(
                7,
                &[(
                    &payload[..0x24 * usize::from(channels) * 3],
                    192,
                    channels,
                    8,
                    &[],
                )],
                1,
                None,
            )?,
        )?;
    }
    Ok(())
}

fn adpcm_edges(
    add: &mut impl FnMut(&str, &[u8]) -> io::Result<()>,
    mode: u32,
    size: usize,
    samples: u32,
    labels: [&str; 10],
    payload: &[u8],
) -> io::Result<()> {
    add(
        labels[4],
        &bank(
            mode,
            &[(&payload[..size * 9], samples * 3, 1, 8, &[(1, &[3])])],
            1,
            None,
        )?,
    )?;
    add(
        labels[5],
        &bank(
            mode,
            &[(&payload[..size * 4 - 1], samples * 2, 2, 8, &[])],
            1,
            None,
        )?,
    )?;
    add(
        labels[6],
        &bank(
            mode,
            &[(&payload[..size * 2 - 1], samples, 2, 8, &[])],
            1,
            None,
        )?,
    )?;
    let mut trailing = bank(mode, &[(&payload[..size], samples * 3, 1, 8, &[])], 1, None)?;
    append(&mut trailing, &payload[..size])?;
    add(labels[7], &trailing)?;
    add(
        labels[8],
        &bank(
            mode,
            &[
                (&payload[..16], samples * 4, 1, 8, &[]),
                (&payload[..size * 2], samples * 2, 1, 8, &[]),
            ],
            1,
            None,
        )?,
    )?;
    add(
        labels[9],
        &bank(mode, &[(&payload[..size], 1 << 20, 1, 8, &[])], 1, None)?,
    )?;
    Ok(())
}

fn pcm_rates(add: &mut impl FnMut(&str, &[u8]) -> io::Result<()>, pcm: &[u8]) -> io::Result<()> {
    let labels = [
        [
            "pcm16_c1_r0",
            "pcm16_c1_r1",
            "pcm16_c1_r2",
            "pcm16_c1_r3",
            "pcm16_c1_r4",
            "pcm16_c1_r5",
            "pcm16_c1_r6",
            "pcm16_c1_r7",
            "pcm16_c1_r8",
            "pcm16_c1_r9",
            "pcm16_c1_r10",
        ],
        [
            "pcm16_c2_r0",
            "pcm16_c2_r1",
            "pcm16_c2_r2",
            "pcm16_c2_r3",
            "pcm16_c2_r4",
            "pcm16_c2_r5",
            "pcm16_c2_r6",
            "pcm16_c2_r7",
            "pcm16_c2_r8",
            "pcm16_c2_r9",
            "pcm16_c2_r10",
        ],
    ];
    for (channels, labels) in (1..=2).zip(labels) {
        for (rate, name) in (0..=10).zip(labels) {
            add(
                name,
                &bank(
                    2,
                    &[(pcm, 8 / u32::from(channels), channels, rate, &[])],
                    1,
                    None,
                )?,
            )?;
        }
    }
    Ok(())
}
