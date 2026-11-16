//! Encoded converter, object-save and own-decoder replay of the pinned oracle.
#![cfg(feature = "encode")]
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
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{build, one_object};
use unityrs_texture::write::{Options, Source, image_to_texture2d, set_image};

fn fixture() -> Json {
    serde_json::from_str(include_str!("fixtures/set_image_encoders.json")).unwrap()
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

/// Decode both byte streams through our decoder so Tier C decoder corrections
/// cannot turn an encoder comparison into a comparison of different readers.
/// Every mip is checked, with the platform transforms only on the top level.
fn compare_levels(case: &Json, actual: &[u8], expected: &[u8], meter: &Arc<Meter>) {
    let mut offset = 0;
    for level in case["levels"].as_array().unwrap() {
        let length = level["length"].as_u64().unwrap() as usize;
        let end = offset + length;
        let blob = level["blob"].as_str().map(hex);
        let mut fields = unityrs_texture::TextureFields {
            texture_format: level["format"].as_i64().unwrap() as i32,
            width: level["size"][0].as_i64().unwrap() as i32,
            height: level["size"][1].as_i64().unwrap() as i32,
            platform: level["platform"].as_i64().unwrap() as i32,
            platform_blob: blob.as_deref(),
        };
        // UnityPy writes ordinary ETC1 while retaining the unsupported 3DS
        // tag. Compare that known payload with the ordinary ETC1 decoder;
        // this does not claim general 3DS decoding support.
        if fields.texture_format == 60 {
            // Platform planning reads the numeric tag even with an explicit
            // decoder, so alias the planner's field as well.
            fields.texture_format = 34;
        }
        let decode = |data: &[u8]| unityrs_texture::decode_in(&fields, data, meter);
        let got = decode(&actual[offset..end]).unwrap();
        let want = decode(&expected[offset..end]).unwrap();
        assert_eq!(got.mode(), want.mode());
        assert_eq!(got.width(), want.width());
        assert_eq!(got.height(), want.height());
        assert_eq!(
            got.pixels(),
            want.pixels(),
            "mip at byte {offset}: {fields:?}"
        );
        offset = end;
    }
    assert_eq!(offset, actual.len());
    assert_eq!(offset, expected.len());
}

fn replay_object(
    fixture: &Json,
    case: &Json,
    source: &Source<'_>,
    options: Options<'_>,
    expected: &[u8],
    index: usize,
    exact: bool,
) {
    let meter = Meter::unlimited();
    for endian in ["<", ">"] {
        let initial = hex(case["initial"][endian].as_str().unwrap());
        let mut spec = one_object(
            28,
            nodes(fixture, case["optional"].as_bool().unwrap()),
            initial.clone(),
        );
        spec.big_endian = endian == ">";
        spec.platform = options.platform;
        let file = load(Bytes::from(build(&spec)), &meter);
        let object = file.object(1).unwrap();
        let bytes = set_image(&object, source, Some(options.format), options.mipmap_count)
            .unwrap_or_else(|error| panic!("object case {index}: {error}"));
        if exact {
            digest(&bytes, &case["object"][endian]);
        } else {
            assert_eq!(
                bytes.len() as u64,
                case["object"][endian]["length"].as_u64().unwrap()
            );
        }
        assert!(file.is_changed());
        assert_eq!(object.raw_data().unwrap().as_ref(), initial);
        let back = load(file.save(SaveOptions::default()).unwrap(), &meter);
        let saved = back.object(1).unwrap();
        assert_eq!(saved.raw_data().unwrap(), bytes);
        let parsed = saved.parse_as_dict(true).unwrap();
        let mut fields = object_fields(&parsed, parsed.value());
        let mut wanted = case["object"]["fields"].clone();
        if !exact {
            fields["image data"]
                .as_object_mut()
                .unwrap()
                .remove("sha256");
            wanted["image data"]
                .as_object_mut()
                .unwrap()
                .remove("sha256");
        }
        assert_eq!(
            fields, wanted,
            "object fields case {index}, endian {endian}"
        );
        let data = parsed.get(&[b"image data"]).unwrap();
        let data = match data {
            Value::Bytes(data) | Value::Prims(PrimArray::U8(data)) => data.as_ref(),
            _ => panic!("unexpected image_data value"),
        };
        compare_levels(case, data, expected, &meter);
    }
    assert_eq!(meter.used(), 0, "object case {index}");
}

#[test]
fn encoded_writes_and_object_saves_replay_the_pinned_oracle() {
    let fixture = fixture();
    let mut targets = std::collections::BTreeSet::new();
    let mut objects = 0;
    let mut mips = 0;
    let mut switch = 0;
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let pixels = hex(
            fixture["sources"][case["source"].as_u64().unwrap() as usize]
                .as_str()
                .unwrap(),
        );
        let palette = case["palette"].as_str().map(|text| {
            let raw = hex(text);
            std::array::from_fn::<_, 256, _>(|n| {
                std::array::from_fn(|channel| raw[n * 4 + channel])
            })
        });
        let source = Source::new(
            case["size"][0].as_u64().unwrap() as u32,
            case["size"][1].as_u64().unwrap() as u32,
            case["mode"].as_str().unwrap(),
            &pixels,
            palette.as_ref(),
        )
        .unwrap();
        let blob = case["blob"].as_str().map(hex);
        let options = Options {
            format: case["format"].as_i64().unwrap() as i32,
            platform: case["platform"].as_i64().unwrap() as i32,
            platform_blob: blob.as_deref(),
            flip: case["flip"].as_bool().unwrap(),
            mipmap_count: case["mips"].as_i64().unwrap() as i32,
        };
        targets.insert(options.format);
        switch += usize::from(options.platform == 38);
        let expected = hex(case["result"]["hex"].as_str().unwrap());
        digest(&expected, &case["result"]);
        let meter = Meter::unlimited();
        let encoded = image_to_texture2d(&source, options, &meter)
            .unwrap_or_else(|error| panic!("case {index}, format {}: {error}", options.format));
        assert_eq!(
            i64::from(encoded.format),
            case["result"]["format"].as_i64().unwrap(),
            "case {index}"
        );
        assert_eq!(
            i64::from(encoded.mipmap_count),
            case["result"]["mips"].as_i64().unwrap(),
            "case {index}"
        );
        assert_eq!(encoded.data.len(), expected.len(), "case {index}");
        mips += usize::from(encoded.mipmap_count > 1);
        let exact = match case["acceptance"].as_str().unwrap() {
            "bytes" => true,
            "decoded" => false,
            other => panic!("unknown encoder acceptance {other}"),
        };
        if exact {
            assert_eq!(
                encoded.data.as_ref(),
                expected,
                "encoded bytes case {index}, format {}",
                options.format
            );
        }
        compare_levels(case, &encoded.data, &expected, &meter);
        drop(encoded);
        assert_eq!(meter.used(), 0, "case {index}");
        if case.get("object").is_none() {
            continue;
        }
        objects += 1;
        replay_object(&fixture, case, &source, options, &expected, index, exact);
    }
    assert_eq!(targets.len(), 36);
    assert_eq!(objects, 352);
    assert_eq!(mips, 76);
    assert_eq!(switch, 29);
}

#[test]
fn encoder_isa_scalar_pin_replays_the_oracle() {
    let fixture: Json = serde_json::from_str(include_str!("fixtures/encoder_isa.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 168);
    for (index, case) in cases.iter().enumerate() {
        let pixels = hex(case["rgba"].as_str().unwrap());
        let source = Source::new(
            case["w"].as_u64().unwrap() as u32,
            case["h"].as_u64().unwrap() as u32,
            "RGBA",
            &pixels,
            None,
        )
        .unwrap();
        let format = case["f"].as_i64().unwrap() as i32;
        let format = if format == 0 {
            let block = case["bx"].as_i64().unwrap();
            assert_eq!(case["bx"], case["by"]);
            48 + i32::try_from(
                [4, 5, 6, 8, 10, 12]
                    .iter()
                    .position(|size| *size == block)
                    .unwrap(),
            )
            .unwrap()
        } else {
            format
        };
        let meter = Meter::unlimited();
        let encoded = image_to_texture2d(
            &source,
            Options {
                flip: false,
                ..Options::new(format)
            },
            &meter,
        )
        .unwrap_or_else(|error| panic!("ISA case {index}, format {format}: {error}"));
        let expected = hex(case["oracle"].as_str().unwrap());
        assert_eq!(
            encoded.data.as_ref(),
            expected,
            "ISA case {index}, format {format}"
        );
        for build in ["scalar", "linux_arm_scalar", "mac-scalar"] {
            assert_eq!(
                case[build], case["oracle"],
                "recorded scalar pin {build}, ISA case {index}"
            );
        }
        drop(encoded);
        assert_eq!(meter.used(), 0, "ISA case {index}");
    }
}
