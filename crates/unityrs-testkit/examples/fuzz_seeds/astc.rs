//! Synthetic ASTC seeds from the pinned block fixture.
use super::{Rng, Seeds, TextureCase};

/// The `astc` seeds' source: `unityrs-texture`'s ASTC oracle fixture.
const ASTC_CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-texture/tests/fixtures/astc.json"
);

/// The `astc` target's seeds: for each of the twelve ASTC LDR formats
/// (`TextureFormat` 48 to 59), the blocks the oracle fixture records for its
/// footprint (astcenc's encoder's, `SplitMix64`'s and the crafted ones), at
/// the fixture's size, under the memory bound and a meter of the peak
/// charge less 1; then a size of 0, a negative side and a byte short of the
/// crafted blocks.
pub(super) fn write(s: &mut Seeds) -> std::io::Result<()> {
    let text = std::fs::read_to_string(ASTC_CASES)?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    let bad = || std::io::Error::other("astc.json: a case is not as the script writes it");
    for (i, side) in [4, 5, 6, 8, 10, 12].into_iter().enumerate() {
        for c in body["footprint_cases"].as_array().ok_or_else(bad)? {
            let id = c["id"].as_str().ok_or_else(bad)?;
            if c["block"] != serde_json::json!([side, side]) {
                continue;
            }
            let data = if let Some(h) = c["data"]["hex"].as_str() {
                h.as_bytes()
                    .chunks(2)
                    .map(|p| {
                        std::str::from_utf8(p)
                            .ok()
                            .and_then(|p| u8::from_str_radix(p, 16).ok())
                            .ok_or_else(bad)
                    })
                    .collect::<std::io::Result<Vec<u8>>>()?
            } else {
                let len = c["data"]["len"].as_u64().ok_or_else(bad)?;
                let mut rng = Rng::new(c["data"]["seed"].as_u64().ok_or_else(bad)?);
                let mut out = Vec::new();
                while u64::try_from(out.len()).unwrap_or(u64::MAX) < len {
                    out.extend_from_slice(&rng.next_u64().to_le_bytes());
                }
                out.truncate(usize::try_from(len).unwrap_or(0));
                out
            };
            let side_of = |key: &str| {
                c[key]
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(bad)
            };
            let (width, height) = (side_of("width")?, side_of("height")?);
            for format in [48 + i, 54 + i] {
                let format = i32::try_from(format).unwrap_or(48);
                let mut add = |tag: &str, width, height, meter, data: &[u8]| {
                    let case = TextureCase {
                        format,
                        width,
                        height,
                        platform: 19,
                        meter,
                        blob: None,
                        data: data[..data.len().min(4096)].to_vec(),
                    };
                    s.add("astc", &format!("f{format}-{id}-{tag}"), &case.encode())
                };
                add("m0", width, height, 0, &data)?;
                add("m1", width, height, 1, &data)?;
                if id.ends_with("-crafted") {
                    add("zero", 0, height, 0, &data)?;
                    add("negative", -width, height, 0, &data)?;
                    add("short", width, height, 0, &data[..data.len() - 1])?;
                }
            }
        }
    }
    Ok(())
}
