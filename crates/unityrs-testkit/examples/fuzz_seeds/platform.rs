//! Platform seeds replay the synthetic oracle bytes, including complete GOBs.
use super::{Seeds, TextureCase};
use serde_json::Value as J;

fn bytes(s: &J) -> Vec<u8> {
    let s = s.as_str().expect("hex");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex byte"))
        .collect()
}
fn int(s: &J) -> i32 {
    i32::try_from(s.as_i64().expect("integer")).expect("i32")
}

pub(super) fn seeds(s: &mut Seeds) -> std::io::Result<()> {
    let doc: J = serde_json::from_str(include_str!(
        "../../../unityrs-texture/tests/fixtures/platform.json"
    ))
    .expect("platform fixture");
    for row in doc["textures"].as_array().expect("textures") {
        for meter in [0, 1] {
            let case = TextureCase {
                format: int(&row["format"]),
                width: int(&row["width"]),
                height: int(&row["height"]),
                platform: int(&row["platform"]),
                blob: (!row["blob"].is_null()).then(|| bytes(&row["blob"])),
                data: bytes(&row["data"]),
                meter,
            };
            s.add(
                "texture",
                &format!("platform-{}-m{meter}", row["id"].as_str().expect("id")),
                &case.encode(),
            )?;
        }
    }
    for exponent in [0, 1, 4, 30, 31, 32, u32::MAX] {
        let mut blob = vec![0; 8];
        blob.extend_from_slice(&exponent.to_le_bytes());
        let case = TextureCase {
            format: 4,
            width: 1,
            height: 1,
            platform: 38,
            blob: Some(blob),
            data: vec![1; 1024],
            meter: 0,
        };
        s.add(
            "texture",
            &format!("platform-gobs-exponent-{exponent}"),
            &case.encode(),
        )?;
    }
    for row in doc["arrays"].as_array().expect("arrays") {
        let mut blob = int(&row["depth"]).to_le_bytes().to_vec();
        blob.extend_from_slice(&row["data_size"].as_u64().expect("size").to_le_bytes());
        let case = TextureCase {
            format: int(&row["graphics_format"]),
            width: int(&row["width"]),
            height: int(&row["height"]),
            platform: int(&row["platform"]),
            blob: Some(blob),
            data: bytes(&row["data"]),
            meter: 0,
        };
        s.add(
            "texture",
            &format!("platform-{}", row["id"].as_str().expect("id")),
            &case.encode(),
        )?;
    }
    Ok(())
}
