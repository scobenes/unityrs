//! Synthetic legacy and Unity fork Crunch seeds, including malformed streams.

use super::Seeds;
use unityrs_testkit::fuzz::TextureCase;

/// Synthetic legacy CRN streams with known BC blocks, checked by the native decoder.
const CRUNCH_CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-texture/tests/fixtures/crunch_native.json"
);

/// Legacy and Unity fork seeds retain whole valid CRN streams at all meter modes,
/// alongside truncations and a broken signature, so mutations start within
/// each header, Huffman, palette and mip-zero path as well as its errors.
pub(super) fn write(s: &mut Seeds) -> std::io::Result<()> {
    let text = std::fs::read_to_string(CRUNCH_CASES)?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    let bad = || std::io::Error::other("crunch_native.json: a case is not as the script writes it");
    let unity: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../unityrs-texture/tests/fixtures/unitycrunch_native.json"
    ))?)
    .map_err(std::io::Error::other)?;
    for c in body["cases"]
        .as_array()
        .ok_or_else(bad)?
        .iter()
        .chain(unity["cases"].as_array().ok_or_else(bad)?)
    {
        let id = c["id"].as_str().ok_or_else(bad)?;
        let format = c["format"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(bad)?;
        let format = format
            + if c["unity"] == true && matches!(format, 28 | 29) {
                100
            } else {
                0
            };
        let side = |key: &str| {
            c[key]
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(bad)
        };
        let data = c["data"]["hex"]
            .as_str()
            .ok_or_else(bad)?
            .as_bytes()
            .chunks(2)
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(bad)
            })
            .collect::<std::io::Result<Vec<u8>>>()?;
        for meter in 0..4 {
            let mut case = TextureCase {
                format,
                width: side("width")?,
                height: side("height")?,
                platform: 19,
                meter,
                blob: None,
                data: data.clone(),
            };
            s.add(
                "crunch",
                &format!("f{format}-{id}-m{meter}"),
                &case.encode(),
            )?;
            for len in [0, 1, 15, 61, data.len().saturating_sub(1)] {
                case.data = data[..len.min(data.len())].to_vec();
                s.add(
                    "crunch",
                    &format!("f{format}-{id}-m{meter}-short{len}"),
                    &case.encode(),
                )?;
            }
            case.data.clone_from(&data);
            if let Some(first) = case.data.first_mut() {
                *first ^= 0xff;
            }
            s.add(
                "crunch",
                &format!("f{format}-{id}-m{meter}-signature"),
                &case.encode(),
            )?;
        }
    }
    Ok(())
}
