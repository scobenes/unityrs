//! HDR seeds replay the same synthetic blocks as the oracle tests.
use std::io;

use serde_json::Value;
use unityrs_testkit::fuzz::TextureCase;

use super::Seeds;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-texture/tests/fixtures/"
);

pub(super) fn write(seeds: &mut Seeds) -> io::Result<()> {
    for (name, astc) in [("bc6h", false), ("astc_hdr", true)] {
        let text = std::fs::read_to_string(format!("{FIXTURES}{name}.json"))?;
        let doc: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
        let bad = || io::Error::other("malformed HDR fixture block");
        for block in doc["blocks"].as_array().ok_or_else(bad)? {
            let data = block["data"].as_str().ok_or_else(bad)?;
            let data = data
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                        .ok_or_else(bad)
                })
                .collect::<io::Result<Vec<_>>>()?;
            let id = block["id"].as_str().ok_or_else(bad)?;
            let format = if astc {
                i32::try_from(block["format"].as_i64().ok_or_else(bad)?)
                    .map_err(io::Error::other)?
            } else {
                24
            };
            let side = if astc {
                i32::try_from(block["side"].as_i64().ok_or_else(bad)?).map_err(io::Error::other)?
            } else {
                4
            };
            for meter in [0, 1] {
                let case = TextureCase {
                    format,
                    width: side,
                    height: side,
                    platform: 19,
                    blob: None,
                    meter,
                    data: data.clone(),
                };
                let label = format!("hdr-{name}-{id}-m{meter}");
                seeds.add("texture", &label, &case.encode())?;
                if astc {
                    seeds.add("astc", &label, &case.encode())?;
                }
            }
        }
    }
    // Reserved BC6H mode codes are absent from the valid-half fixture.
    for mode in [19, 23, 27, 31] {
        for meter in [0, 1] {
            let mut data = vec![0; 16];
            data[0] = mode;
            let case = TextureCase {
                format: 24,
                width: 4,
                height: 4,
                platform: 19,
                blob: None,
                meter,
                data,
            };
            seeds.add(
                "texture",
                &format!("hdr-bc6h-reserved-{mode}-m{meter}"),
                &case.encode(),
            )?;
        }
    }
    arrays(seeds)
}

/// Each slice has one FP16 void-extent top block followed by a deliberately
/// invalid mip block. Array decoding must stride over that ignored mip tail.
fn arrays(seeds: &mut Seeds) -> io::Result<()> {
    let mut blob = 2_i32.to_le_bytes().to_vec();
    blob.extend_from_slice(&64_u64.to_le_bytes());
    let mut data = vec![0; 64];
    for (offset, half) in [
        (0, [0x3c00_u16, 0x3800, 0x3400, 0x3c00]),
        (32, [0, 0x3400, 0x3800, 0x3c00]),
    ] {
        data[offset..offset + 8].fill(0xff);
        data[offset] = 0xfc;
        for (channel, value) in half.into_iter().enumerate() {
            let start = offset + 8 + channel * 2;
            data[start..start + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    for (format, side) in (145..=150).zip([4, 5, 6, 8, 10, 12]) {
        for (label, platform, meter, malformed) in [
            ("valid-m0", 19, 0, false),
            ("valid-m1", 19, 1, false),
            ("malformed", 19, 0, true),
            ("android", 13, 0, false),
            ("switch", 38, 0, false),
            ("xbox360", 11, 0, false),
        ] {
            let mut data = data.clone();
            if malformed {
                data[..16].fill(0);
            }
            let case = TextureCase {
                format,
                width: side,
                height: side,
                platform,
                blob: Some(blob.clone()),
                meter,
                data,
            };
            seeds.add(
                "texture",
                &format!("array-astc-{format}-{label}"),
                &case.encode(),
            )?;
        }
    }
    Ok(())
}
