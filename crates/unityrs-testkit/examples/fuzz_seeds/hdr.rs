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
    Ok(())
}
