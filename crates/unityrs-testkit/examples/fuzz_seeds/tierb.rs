//! The texture target's Tier B/C seeds, from the synthetic oracle fixture:
//! exact and short data, IEEE specials, rounding neighbours, all shared
//! exponents and the Tier U refusals, with both meter settings.
use std::io;

use serde_json::Value;
use unityrs_testkit::Rng;
use unityrs_testkit::fuzz::TextureCase;

use super::Seeds;

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-texture/tests/fixtures/tierb.json"
);

pub(super) fn write(seeds: &mut Seeds) -> io::Result<()> {
    let text = std::fs::read_to_string(CASES)?;
    let doc: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
    let bad = || io::Error::other("tierb.json: malformed case");
    for c in doc["cases"].as_array().ok_or_else(bad)? {
        let id = c["id"].as_str().ok_or_else(bad)?;
        // Exhaustive ramps are replayed by the native tests; the fuzz
        // target starts with small, fully present images instead.
        if id.contains("-every-") {
            continue;
        }
        let data = build(&c["data"])?;
        let number = |key: &str| {
            c[key]
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(bad)
        };
        for meter in [0, 1] {
            let case = TextureCase {
                format: number("format")?,
                width: number("width")?,
                height: number("height")?,
                platform: 19,
                meter,
                blob: None,
                data: data.clone(),
            };
            seeds.add("texture", &format!("tierb-{id}-m{meter}"), &case.encode())?;
        }
    }
    Ok(())
}

fn build(spec: &Value) -> io::Result<Vec<u8>> {
    let bad = || io::Error::other("tierb.json: malformed data");
    if let Some(hex) = spec["hex"].as_str() {
        return hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(bad)
            })
            .collect();
    }
    let n = spec["zeros"]
        .as_u64()
        .or_else(|| spec["len"].as_u64())
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(bad)?;
    if spec["zeros"].is_number() {
        return Ok(vec![0; n]);
    }
    let mut rng = Rng::new(spec["seed"].as_u64().ok_or_else(bad)?);
    let mut data = Vec::new();
    while data.len() < n {
        data.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    data.truncate(n);
    Ok(data)
}
