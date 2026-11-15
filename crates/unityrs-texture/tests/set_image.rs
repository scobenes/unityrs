//! Synthetic UnityPy/Pillow write replay, including B1 object and file save.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation
)]
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::sync::Arc;
use unityrs_core::files::{SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, Parsed, PrimArray, RawStr, Value};
use unityrs_imageio::writers::resample::{Filter, resize};
use unityrs_imageio::{Image, Mode, Unmetered};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{build, one_object};
use unityrs_texture::write::{Options, Source, WriteError, image_to_texture2d, set_image};

fn fixture() -> Json {
    serde_json::from_str(include_str!("fixtures/set_image.json")).unwrap()
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn digest(bytes: &[u8], expected: &Json) {
    assert_eq!(bytes.len() as u64, expected["length"].as_u64().unwrap());
    assert_eq!(
        Sha256::digest(bytes).as_slice(),
        hex(expected["sha256"].as_str().unwrap())
    );
}
fn load(bytes: Bytes, meter: &Arc<Meter>) -> Arc<SerializedFile> {
    SerializedFile::parse(
        RawStr::from_static(b"CAB-synthetic-write"),
        bytes,
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}
fn nodes(fixture: &Json, optional: bool) -> Vec<NodeSpec> {
    fixture["schema"][if optional { "full" } else { "minimal" }]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let mut node = NodeSpec::new(
                row[0].as_u64().unwrap() as u8,
                row[1].as_str().unwrap(),
                row[2].as_str().unwrap(),
            )
            .size(-1);
            node.meta_flag = row[3].as_i64().unwrap() as i32;
            node
        })
        .collect()
}

fn object_fields(parsed: &Parsed, value: &Value) -> Json {
    match value {
        Value::Bool(v) => Json::from(*v),
        Value::Int(v) => Json::from(*v),
        Value::UInt(v) => Json::from(*v),
        Value::Str(v) => Json::from(std::str::from_utf8(v.as_bytes()).unwrap()),
        Value::Bytes(v) | Value::Prims(PrimArray::U8(v)) => serde_json::json!({
            "length": v.len(), "sha256": Sha256::digest(v).iter()
                .fold(String::new(), |mut text, byte| {
                    write!(text, "{byte:02x}").unwrap();
                    text
                })
        }),
        Value::Dict(map) => Json::Object(
            parsed
                .entries(map)
                .map(|(name, value)| {
                    (
                        std::str::from_utf8(name).unwrap().to_owned(),
                        object_fields(parsed, value),
                    )
                })
                .collect(),
        ),
        _ => panic!("unexpected texture field {value:?}"),
    }
}

fn check_refusal(
    policy: &str,
    result: &Result<unityrs_texture::write::Encoded, WriteError>,
    i: usize,
) {
    if policy == "encoder" {
        assert!(
            matches!(result, Err(WriteError::EncoderUnavailable(_))),
            "case {i}: {result:?}"
        );
    } else {
        let expected = match policy {
            "unsupported_mode" => "Pillow cannot convert image mode to RGBA",
            "switch_mips" => "Switch mip chain",
            "switch_empty" => "zero-size Switch texture",
            "switch_format" => "Switch texel block",
            "xbox" => "Xbox byte-swapped write",
            _ => panic!("unrecorded policy {policy}"),
        };
        assert!(
            matches!(result, Err(WriteError::Unsupported(reason)) if *reason == expected),
            "case {i}: {result:?}"
        );
    }
}

#[test]
fn every_raw_write_and_object_save_replays_the_oracle() {
    let fixture = fixture();
    let mut replayed = 0;
    let mut policies = std::collections::BTreeMap::<String, usize>::new();
    for (i, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let width = case["size"][0].as_u64().unwrap() as u32;
        let height = case["size"][1].as_u64().unwrap() as u32;
        let pixels = hex(
            fixture["sources"][case["source"].as_u64().unwrap() as usize]
                .as_str()
                .unwrap(),
        );
        let palette = case["palette"].as_str().map(|s| {
            let raw = hex(s);
            std::array::from_fn::<_, 256, _>(|n| std::array::from_fn(|c| raw[n * 4 + c]))
        });
        let mut source = Source::new(
            width,
            height,
            case["mode"].as_str().unwrap(),
            &pixels,
            palette.as_ref(),
        )
        .unwrap();
        let auxiliary = case["auxiliary"]
            .as_u64()
            .map(|v| vec![v as u8; width as usize * height as usize]);
        if let Some(auxiliary) = &auxiliary {
            source = source.with_auxiliary(auxiliary).unwrap();
        }
        let blob = case["blob"].as_str().map(hex);
        let options = Options {
            format: case["format"].as_i64().unwrap() as i32,
            platform: case["platform"].as_i64().unwrap() as i32,
            platform_blob: blob.as_deref(),
            flip: case["flip"].as_bool().unwrap(),
            mipmap_count: case["mips"].as_i64().unwrap() as i32,
        };
        let meter = Meter::unlimited();
        let result = image_to_texture2d(&source, options, &meter);
        let policy = case["policy"].as_str().unwrap_or("oracle");
        *policies.entry(policy.to_owned()).or_default() += 1;
        if case.get("result").is_none() {
            check_refusal(policy, &result, i);
            assert_eq!(meter.used(), 0, "case {i}");
            continue;
        }
        let encoded = result.unwrap_or_else(|e| panic!("case {i} {case}: {e}"));
        digest(&encoded.data, &case["result"]);
        assert_eq!(
            i64::from(encoded.format),
            case["result"]["format"].as_i64().unwrap(),
            "case {i}"
        );
        assert_eq!(
            i64::from(encoded.mipmap_count),
            case["result"]["mips"].as_i64().unwrap(),
            "case {i}"
        );
        drop(encoded);
        assert_eq!(meter.used(), 0, "case {i}");
        if case.get("object").is_none() {
            continue;
        }
        replayed += 1;
        for endian in ["<", ">"] {
            let initial = hex(case["initial"][endian].as_str().unwrap());
            let mut spec = one_object(
                28,
                nodes(&fixture, case["optional"].as_bool().unwrap()),
                initial.clone(),
            );
            spec.big_endian = endian == ">";
            spec.platform = options.platform;
            let file = load(Bytes::from(build(&spec)), &meter);
            let object = file.object(1).unwrap();
            assert!(!file.is_changed());
            let bytes = set_image(&object, &source, Some(options.format), options.mipmap_count)
                .unwrap_or_else(|e| panic!("object case {i}: {e}"));
            digest(&bytes, &case["object"][endian]);
            assert!(file.is_changed());
            // B1 preserves the original reader. Installed bytes appear on save.
            assert_eq!(object.raw_data().unwrap().as_ref(), initial);
            let back = load(file.save(SaveOptions::default()).unwrap(), &meter);
            let saved = back.object(1).unwrap();
            assert_eq!(saved.raw_data().unwrap(), bytes);
            let parsed = saved.parse_as_dict(true).unwrap();
            assert_eq!(
                object_fields(&parsed, parsed.value()),
                case["object"]["fields"],
                "object fields case {i}, endian {endian}"
            );
        }
        assert_eq!(meter.used(), 0, "object case {i}");
    }
    assert!(replayed > 600, "{replayed}, {policies:?}");
    assert!(policies["encoder"] > 50);
    assert_eq!(policies["switch_mips"], 3);
}

#[test]
fn bicubic_replays_pillow_fixed_point_and_premultiplied_alpha() {
    let fixture = fixture();
    let rows = fixture["resizes"].as_array().unwrap();
    assert_eq!(rows.len(), 35);
    for case in rows {
        let pixels = hex(case["source"].as_str().unwrap());
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let image = Image::new(
            case["size"][0].as_u64().unwrap() as u32,
            case["size"][1].as_u64().unwrap() as u32,
            mode,
            &pixels,
        )
        .unwrap();
        let output = resize(
            &image,
            case["dest"][0].as_u64().unwrap() as u32,
            case["dest"][1].as_u64().unwrap() as u32,
            Filter::Bicubic,
            &mut Unmetered,
        )
        .unwrap();
        assert_eq!(output.bytes, hex(case["hex"].as_str().unwrap()), "{case}");
    }
}
