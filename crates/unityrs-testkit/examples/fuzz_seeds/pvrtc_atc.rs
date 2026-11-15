//! Synthetic PVRTC/ATC texture seeds; no oracle file required.
use super::{Rng, Seeds, TextureCase};

pub(super) fn write(s: &mut Seeds) -> std::io::Result<()> {
    let mut rng = Rng::new(0x5056_5254_4341_5443);
    for format in [30, 31, 32, 33, 35, 36] {
        let bw = if format < 32 { 8 } else { 4 };
        let bytes = if format == 36 { 16 } else { 8 };
        for (width, height) in [
            (1, 1),
            (3, 5),
            (bw, 4),
            (2 * bw, 8),
            (4 * bw - 1, 7),
            (bw, 32),
            (8 * bw, 4),
            (3 * bw, 8),
            (bw, 12),
            (0, 8),
            (-1, 4),
        ] {
            let n = if width > 0 && height > 0 {
                usize::try_from((width + bw - 1) / bw * ((height + 3) / 4) * bytes).unwrap_or(0)
            } else {
                0
            };
            for (tag, len) in [
                ("empty", 0),
                ("short", n.saturating_sub(1)),
                ("exact", n),
                ("long", n + 32),
            ] {
                let mut data = Vec::new();
                while data.len() < len {
                    data.extend_from_slice(&rng.next_u64().to_le_bytes());
                }
                data.truncate(len);
                for meter in [0, 1] {
                    let case = TextureCase {
                        format,
                        width,
                        height,
                        meter,
                        platform: 19,
                        blob: None,
                        data: data.clone(),
                    };
                    s.add(
                        "texture",
                        &format!("pvrtc-atc-f{format}-{width}x{height}-{tag}-m{meter}"),
                        &case.encode(),
                    )?;
                }
            }
        }
        // All PVRTC modulation modes, with opaque/transparent endpoints;
        // for ATC, both colour and both alpha endpoint orders.
        for mode in 0..8 {
            let mut data = vec![0xff; usize::try_from(bytes).unwrap_or(0)];
            if format < 34 {
                data[0] = u8::from(mode & 2 != 0);
                data[2] = if mode & 4 == 0 { 0 } else { 16 };
                data[4] = mode & 1;
            } else {
                let start = if format == 36 { 8 } else { 0 };
                data[start + 1] = if mode & 1 == 0 { 0x7f } else { 0xff };
                if format == 36 {
                    data[0] = if mode & 2 == 0 { 0 } else { 255 };
                    data[1] = 120;
                }
            }
            let case = TextureCase {
                format,
                width: bw,
                height: 4,
                meter: 0,
                platform: 19,
                blob: None,
                data,
            };
            s.add(
                "texture",
                &format!("pvrtc-atc-f{format}-mode-{mode}"),
                &case.encode(),
            )?;
        }
    }
    Ok(())
}
