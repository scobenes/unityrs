//! Replays the pinned UnityPy platform helpers, conversion and array property.
use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_texture::array::ArrayError as E;
use unityrs_texture::{Mode, TextureFields, array, decode_in, pad_image, plan_in, platform};

fn doc() -> J {
    serde_json::from_str(include_str!("fixtures/platform.json")).unwrap()
}
fn bytes(s: &J) -> Vec<u8> {
    let s = s.as_str().unwrap();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn int(j: &J) -> i32 {
    i32::try_from(j.as_i64().unwrap()).unwrap()
}
fn digest(b: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(b).iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

fn image(image: &unityrs_texture::Image, want: &J, id: &str) {
    assert_eq!(image.mode().name(), want["mode"], "{id}");
    assert_eq!(
        serde_json::json!([image.width(), image.height()]),
        want["size"],
        "{id}"
    );
    assert_eq!(digest(image.pixels()), want["sha256"], "{id}");
}

#[test]
fn switch_gob_constants_and_all_29_texel_blocks_replay_the_oracle() {
    let doc = doc();
    let meter = Meter::unlimited();
    let mut formats = std::collections::BTreeSet::new();
    for c in doc["textures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| !c["block"].is_null())
    {
        let fmt = int(&c["format"]);
        formats.insert(fmt);
        let blob = bytes(&c["blob"]);
        let exponent = u32::from_le_bytes(blob[8..12].try_into().unwrap());
        let layout =
            platform::switch_layout(fmt, int(&c["width"]), int(&c["height"]), exponent).unwrap();
        assert_eq!(serde_json::json!(layout.padded_size()), c["padded"]);
        assert_eq!(serde_json::json!(layout.block()), c["block"]);
        let linear = platform::deswizzle(&bytes(&c["data"]), &layout, &meter).unwrap();
        assert_eq!(digest(linear.as_slice()), c["linear_sha256"], "{}", c["id"]);
        drop(linear);
        assert_eq!(meter.used(), 0);
    }
    assert_eq!(formats.len(), 29);
}

#[test]
fn switch_padding_crop_promotions_xbox_swap_and_crunch_order_replay() {
    let doc = doc();
    let mut images = 0;
    let mut refusals = 0;
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["textures"].as_array().unwrap().len(), 182);
    for c in doc["textures"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let blob = (!c["blob"].is_null()).then(|| bytes(&c["blob"]));
        let fields = TextureFields {
            texture_format: int(&c["format"]),
            width: int(&c["width"]),
            height: int(&c["height"]),
            platform: int(&c["platform"]),
            platform_blob: blob.as_deref(),
        };
        let data = bytes(&c["data"]);
        let meter = Meter::unlimited();
        let planned = plan_in(&fields, data.len());
        let got = decode_in(&fields, &data, &meter);
        if matches!(fields.texture_format, 9 | 62) && !c["block"].is_null() {
            // T7 corrects the raw conversion after T6 deswizzles it. Keep
            // UnityPy's recorded exception, and independently read the high
            // byte (R16) or both stored bytes (RG16), crop, then flip.
            assert!(c["raises"].is_string(), "{id}");
            let exponent = u32::from_le_bytes(blob.as_ref().unwrap()[8..12].try_into().unwrap());
            let layout = platform::switch_layout(
                fields.texture_format,
                fields.width,
                fields.height,
                exponent,
            )
            .unwrap();
            let linear = platform::deswizzle(&data, &layout, &meter).unwrap();
            let mut expected = Vec::new();
            let stride = layout.padded_size().0 as usize * 2;
            for y in (0..usize::try_from(fields.height).unwrap()).rev() {
                for x in 0..usize::try_from(fields.width).unwrap() {
                    let p = &linear.as_slice()[y * stride + x * 2..][..2];
                    let pixel = if fields.texture_format == 9 {
                        [p[1], 0, 0]
                    } else {
                        [p[0], p[1], 0]
                    };
                    expected.extend_from_slice(&pixel);
                }
            }
            drop(linear);
            let got = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            let p = planned.unwrap();
            assert_eq!(got.mode(), Mode::Rgb, "{id}");
            assert_eq!(got.pixels(), expected, "{id}");
            assert_eq!(
                (got.width(), got.height()),
                (fields.width.unsigned_abs(), fields.height.unsigned_abs())
            );
            assert_eq!(meter.used(), p.charge());
            drop(got);
            assert_eq!(meter.used(), 0);
            images += 1;
            continue;
        }
        if c["mode"].is_null() {
            let error = got.err().unwrap_or_else(|| panic!("{id}: decoded"));
            let expected = if fields.texture_format == 24 {
                // The oracle still raises "bad image mode". The linked
                // specification decoder reaches a reserved mode in these
                // synthetic blocks after the platform plan succeeds.
                assert_eq!(c["raises"], "ValueError", "{id}");
                assert_eq!(c["message"], "bad image mode", "{id}");
                assert!(planned.is_ok(), "{id}");
                unityrs_texture::DecodeError::Hdr(unityrs_texture::HdrError::ReservedMode)
            } else if fields.texture_format == 6 {
                unityrs_texture::DecodeError::Unsupported {
                    format: unityrs_texture::Format::ArgbFloat,
                    width: fields.width,
                }
            } else {
                assert!(c["block"].is_null(), "{id}");
                unityrs_texture::DecodeError::Platform(
                    unityrs_texture::PlatformError::SwitchFormat(fields.texture_format),
                )
            };
            assert_eq!(error, expected, "{id}");
            if fields.texture_format != 24 {
                assert_eq!(planned.unwrap_err(), expected, "{id}");
            }
            refusals += 1;
        } else {
            let got = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            let p = planned.unwrap();
            image(&got, c, id);
            assert_eq!(fields.image_mode(), Some(got.mode()), "{id}");
            assert_eq!(p.output_bytes, got.pixels().len() as u64);
            assert_eq!(meter.used(), p.charge(), "{id}");
            drop(got);
            images += 1;
        }
        assert_eq!(meter.used(), 0, "{id}");
    }
    assert_eq!((images, refusals), (129, 53));
}

#[test]
fn pad_image_repeats_nearest_edges_and_returns_the_same_image_at_its_size() {
    let doc = doc();
    for c in doc["pads"].as_array().unwrap() {
        let meter = Meter::unlimited();
        let mode = match c["mode"].as_str().unwrap() {
            "L" => Mode::L,
            "RGB" => Mode::Rgb,
            _ => Mode::Rgba,
        };
        let w = int(&c["width"]);
        let h = int(&c["height"]);
        let input = bytes(&c["data"]);
        let original = unityrs_texture::image_from_pixels(
            mode,
            u32::try_from(w).unwrap(),
            u32::try_from(h).unwrap(),
            &input,
            &meter,
        )
        .unwrap();
        let ptr = original.pixels().as_ptr();
        let out = pad_image(
            original,
            u32::try_from(int(&c["padded"][0])).unwrap(),
            u32::try_from(int(&c["padded"][1])).unwrap(),
            &meter,
        );
        if c["image"]["raises"].is_string() {
            assert!(matches!(
                c["image"]["raises"].as_str(),
                Some("IndexError" | "ValueError")
            ));
            assert_eq!(
                out.err().unwrap(),
                unityrs_texture::DecodeError::Platform(unityrs_texture::PlatformError::PaddingSize)
            );
            assert_eq!(meter.used(), 0);
            continue;
        }
        let out = out.unwrap();
        image(&out, &c["image"], "pad");
        if c["same"] == true {
            assert_eq!(ptr, out.pixels().as_ptr());
        }
        drop(out);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn xbox_pair_swap_preserves_an_odd_final_byte_and_empty_input() {
    let doc = doc();
    let meter = Meter::unlimited();
    for row in doc["swaps"].as_array().unwrap() {
        let data = bytes(&row["data"]);
        let out = platform::swap_bytes_for_xbox(&data, &meter).unwrap();
        assert_eq!(out.as_slice(), bytes(&row["swapped"]));
        drop(out);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn texture2darray_map_slices_integer_stream_divergences_and_platform_refusals_replay() {
    let doc = doc();
    for pair in doc["graphics_map"].as_array().unwrap() {
        assert_eq!(
            array::unitypy_texture_format(int(&pair[0])).unwrap(),
            int(&pair[1])
        );
    }
    assert_eq!(doc["arrays"].as_array().unwrap().len(), 111);
    for c in doc["arrays"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let fields = array::ArrayFields {
            graphics_format: int(&c["graphics_format"]),
            width: int(&c["width"]),
            height: int(&c["height"]),
            depth: int(&c["depth"]),
            data_size: c["data_size"].as_u64().unwrap(),
            platform: int(&c["platform"]),
        };
        let data = bytes(&c["data"]);
        let (inline, resource) = if c["streamed"] == true {
            (&[][..], Some(data.as_slice()))
        } else {
            (data.as_slice(), None)
        };
        let meter = Meter::unlimited();
        let got = array::decode(&fields, inline, resource, &meter);
        if matches!(fields.platform, 11 | 38)
            || c["oracle"]["raises"].is_string() && c["reference"].is_null()
        {
            let error = got.err().unwrap_or_else(|| panic!("{id}: decoded"));
            let expected = if matches!(fields.platform, 11 | 38) {
                E::UnsupportedPlatform(fields.platform)
            } else {
                match id {
                    "array-unknown" | "array-zero-unknown" => E::UnknownGraphicsFormat(999),
                    "array-unmapped" | "array-zero-unmapped" => E::UnmappedGraphicsFormat(0),
                    "array-depth-zero"
                    | "array-step-zero"
                    | "array-zero-depth-zero"
                    | "array-zero-step-zero" => E::Layout,
                    "array-remainder" => {
                        E::Decode(unityrs_texture::DecodeError::NotEnoughData { needed: 4, got: 1 })
                    }
                    "array-short" => E::Decode(unityrs_texture::DecodeError::NotEnoughData {
                        needed: 24,
                        got: 1,
                    }),
                    _ => panic!("unmapped oracle exception in {id}"),
                }
            };
            assert_eq!(error, expected, "{id}");
            assert_eq!(
                array::plan(&fields, data.len()).unwrap_err(),
                expected,
                "{id}"
            );
        } else {
            let want = if c["reference"].is_null() {
                &c["oracle"]
            } else {
                &c["reference"]
            };
            let got = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            let p = array::plan(&fields, data.len()).unwrap();
            assert!(meter.used() <= p.peak_charge, "{id}");
            let want = want["images"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{id}");
            assert_eq!(p.slice_count, want.len(), "{id}");
            if fields.width == 0 || fields.height == 0 {
                assert_eq!(p.image.mode, Mode::Rgba, "{id}");
                assert_eq!((p.image.width, p.image.height), (0, 0), "{id}");
                assert_eq!(
                    (p.image.input_bytes, p.image.output_bytes, p.image.scratch),
                    (0, 0, 0),
                    "{id}"
                );
            }
            for (got, want) in got.as_slice().iter().zip(want) {
                image(got, want, id);
            }
            drop(got);
        }
        assert_eq!(meter.used(), 0, "{id}");
    }
}
