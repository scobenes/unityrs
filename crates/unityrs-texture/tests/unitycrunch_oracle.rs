//! Unity fork streams from the original synthetic builder. Oracle pixel
//! replay remains explicit when the coordinator's pinned fixture is absent.
use serde_json::Value;
use unityrs_core::limits::Meter;
use unityrs_texture::{Format, Selection, TextureFields, decode, select_crunch};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn format(case: &Value) -> Format {
    let fields = TextureFields {
        texture_format: i32::try_from(case["format"].as_i64().unwrap()).unwrap(),
        width: 4,
        height: 4,
        platform: 19,
        platform_blob: None,
    };
    let Selection::Export(f) = select_crunch(&fields, (2017, 3)) else {
        panic!("not selected")
    };
    f
}
#[test]
fn fork_streams_decode_as_independently_constructed_blocks() {
    let doc: Value =
        serde_json::from_str(include_str!("fixtures/unitycrunch_native.json")).unwrap();
    assert!(doc["cases"].as_array().unwrap().len() >= 39);
    for case in doc["cases"].as_array().unwrap() {
        let data = unhex(case["data"]["hex"].as_str().unwrap());
        let blocks = unhex(case["levels"][0]["blocks"].as_str().unwrap());
        let plain = match case["format"].as_i64().unwrap() {
            28 => Format::Dxt1,
            29 => Format::Dxt5,
            64 => Format::EtcRgb4,
            65 => Format::Etc2Rgba8,
            _ => panic!(),
        };
        let (w, h) = (
            i32::try_from(case["width"].as_i64().unwrap()).unwrap(),
            i32::try_from(case["height"].as_i64().unwrap()).unwrap(),
        );
        let meter = Meter::unlimited();
        let image = decode(format(case), &data, w, h, &meter)
            .unwrap_or_else(|e| panic!("{}: {e}", case["id"]));
        let expected = decode(plain, &blocks, w, h, &Meter::unlimited()).unwrap();
        assert_eq!(image.pixels(), expected.pixels(), "{}", case["id"]);
        drop(image);
        assert_eq!(meter.used(), 0);
    }
}
#[test]
fn fork_pixels_replay_the_pinned_unitypy_oracle() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crunch.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!(
            "SKIP Unity Crunch oracle: regenerate tools/fixtures/texture_crunch.py in the pinned container"
        );
        return;
    };
    let doc: Value = serde_json::from_str(&text).unwrap();
    let rows = doc["unity_cases"].as_array().expect(
        "crunch.json records unity_cases; regenerate it with tools/fixtures/texture_crunch.py",
    );
    assert_eq!(doc["texture2ddecoder"], "1.0.5");
    assert_eq!(doc["unitypy"], "1.25.2");
    let native: Value =
        serde_json::from_str(include_str!("fixtures/unitycrunch_native.json")).unwrap();
    assert_eq!(rows.len(), native["cases"].as_array().unwrap().len());
    for (case, expected) in rows.iter().zip(native["cases"].as_array().unwrap()) {
        for key in [
            "id",
            "format",
            "width",
            "height",
            "data",
            "levels",
            "data_sha256",
        ] {
            assert_eq!(case[key], expected[key], "{key}");
        }
        let image = decode(
            format(case),
            &unhex(case["data"]["hex"].as_str().unwrap()),
            i32::try_from(case["width"].as_i64().unwrap()).unwrap(),
            i32::try_from(case["height"].as_i64().unwrap()).unwrap(),
            &Meter::unlimited(),
        )
        .unwrap();
        assert_eq!(image.mode().name(), case["mode"].as_str().unwrap());
        assert_eq!(
            image.pixels(),
            unhex(case["pixels"].as_str().unwrap()),
            "{}",
            case["id"]
        );
    }
}
