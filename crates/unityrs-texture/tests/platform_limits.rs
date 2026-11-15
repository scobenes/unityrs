//! Hostile metadata and every platform allocation/work boundary.
use std::sync::Arc;
use unityrs_core::{
    Bytes, File, RawStr,
    budget::{Reservation, Unbounded},
    env::{EnvOptions, Environment, Source},
    limits::{ChargeError, Meter, heap_cost},
};
use unityrs_testkit::ngui::{Object, V, file_on};
use unityrs_texture::{
    DecodeError, Format, Formats, PlatformError, Selection, Skip, SkippedPlatform, TextureFields,
    array::{self, ArrayError, ArrayFields},
    decode_in, plan_in, platform, select_in,
};

fn texture(blob: &[u8]) -> TextureFields<'_> {
    TextureFields {
        texture_format: 4,
        width: 3,
        height: 2,
        platform: 38,
        platform_blob: Some(blob),
    }
}

#[test]
fn platform_refusals_and_size_overflow_are_typed_before_allocation() {
    let meter = Meter::unlimited();
    for exponent in [31u32, 32, 63, u32::MAX] {
        let mut blob = vec![0; 8];
        blob.extend_from_slice(&exponent.to_le_bytes());
        let fields = texture(&blob);
        assert!(matches!(
            decode_in(&fields, &[1; 16], &meter),
            Err(DecodeError::TooLarge { .. }
                | DecodeError::Platform(PlatformError::GobsExponent(_)))
        ));
        assert_eq!(meter.used(), 0);
    }
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    let mut fields = texture(&blob);
    for fmt in [11, 28, 29, 34, 41, 47, 64, 65, 66, 82] {
        fields.texture_format = fmt;
        assert!(
            matches!(plan_in(&fields, 16), Err(DecodeError::Platform(PlatformError::SwitchFormat(f))) if f == fmt)
        );
    }
    fields.texture_format = 4;
    fields.width = -1;
    assert!(matches!(
        plan_in(&fields, 16),
        Err(DecodeError::NegativeSize { .. })
    ));
    assert_eq!(plan_in(&fields, 0).unwrap_err(), DecodeError::NoImageData);
    fields.width = 0;
    assert_eq!(decode_in(&fields, &[], &meter).unwrap().pixels(), &[]);
    fields.width = i32::MAX;
    fields.height = i32::MAX;
    assert!(matches!(
        plan_in(&fields, 1),
        Err(DecodeError::TooLarge { .. })
    ));
    fields = texture(&blob);
    assert_eq!(
        plan_in(&fields, 1).unwrap_err(),
        DecodeError::NotEnoughData {
            needed: 1024,
            got: 1
        }
    );
    assert_eq!(meter.used(), 0);
}

#[test]
fn supported_platforms_select_and_switch_without_a_texel_block_refuses() {
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    for format in Format::ALL {
        let allowed = Formats::NONE.with(format);
        let fields = TextureFields {
            texture_format: format.texture_format(),
            width: 4,
            ..texture(&blob)
        };
        let expected = if platform::switch_block(match fields.texture_format {
            3 => 4,
            8 => 14,
            other => other,
        })
        .is_some()
        {
            Selection::Export(format)
        } else {
            Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
        };
        assert_eq!(select_in(&fields, allowed, (2017, 2)), expected);
        assert_eq!(
            select_in(
                &TextureFields {
                    platform: 11,
                    ..fields
                },
                allowed,
                (2017, 2)
            ),
            Selection::Export(format)
        );
    }
}

#[test]
fn transform_padded_decode_and_crop_charge_before_allocating_and_release() {
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    let fields = texture(&blob);
    let data = [9u8; 1024];
    let p = plan_in(&fields, data.len()).unwrap();
    let transform = heap_cost(data.len() as u64);
    for limit in [
        0,
        transform - 1,
        transform,
        transform * 2 - 1,
        transform * 2,
        p.peak_charge() - 1,
        p.peak_charge(),
    ] {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        match decode_in(&fields, &data, &meter) {
            Ok(image) => {
                assert_eq!(limit, p.peak_charge());
                assert_eq!(meter.used(), p.charge());
                drop(image);
            }
            Err(e) => assert!(matches!(e, DecodeError::Charge(_)), "{limit}: {e}"),
        }
        assert_eq!(meter.used(), 0);
    }
    for platform in [11, 38] {
        let fields = TextureFields {
            texture_format: if platform == 11 { 7 } else { 4 },
            platform,
            ..fields
        };
        let meter = Meter::with_decode_work(u64::MAX, 1, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            decode_in(&fields, &data, &meter),
            Err(DecodeError::Charge(ChargeError::Limit(_)))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn arrays_bound_depth_step_size_output_and_work() {
    let mut fields = ArrayFields {
        graphics_format: 4,
        width: 1,
        height: 1,
        depth: 2,
        data_size: 8,
        platform: 13,
    };
    let data = [3u8; 8];
    for depth in [0, -1, i32::MIN] {
        fields.depth = depth;
        assert_eq!(array::plan(&fields, 8).unwrap_err(), ArrayError::Layout);
    }
    fields.depth = 2;
    fields.data_size = 1;
    assert_eq!(array::plan(&fields, 8).unwrap_err(), ArrayError::Layout);
    fields.data_size = u64::MAX;
    assert!(array::plan(&fields, 8).is_err());
    fields.data_size = 8;
    let p = array::plan(&fields, data.len()).unwrap();
    for limit in [0, p.peak_charge - 1, p.peak_charge] {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let result = array::decode(&fields, &data, None, &meter);
        assert_eq!(result.is_ok(), limit == p.peak_charge);
        drop(result);
        assert_eq!(meter.used(), 0);
    }
    let meter = Meter::with_decode_work(u64::MAX, 1, Arc::new(Unbounded), Reservation::Input);
    assert!(array::decode(&fields, &data, None, &meter).is_err());
    assert_eq!(meter.used(), 0);
}

#[test]
fn corrected_array_byte_formats_use_closed_form_bounds_instead_of_pillow_caps() {
    for graphics_format in [14, 18] {
        let fields = ArrayFields {
            graphics_format,
            width: 100_000_000,
            height: 1,
            depth: 1,
            data_size: 200_000_000,
            platform: 13,
        };
        let p = array::plan(&fields, 200_000_000).unwrap();
        assert_eq!(p.image.input_bytes, 200_000_000);
        assert_eq!(p.image.output_bytes, 300_000_000);
        assert_eq!(p.image.scratch, 0);
    }
}

#[test]
fn an_array_object_resolves_its_empty_inline_data_from_the_environment() {
    let data = vec![10, 20, 30, 40, 50, 60, 70, 80];
    let object = Object::new(
        1,
        187,
        V::Struct(
            "Texture2DArray".into(),
            vec![
                ("m_Format".into(), V::Int(4)),
                ("m_Width".into(), V::Int(1)),
                ("m_Height".into(), V::Int(1)),
                ("m_Depth".into(), V::Int(2)),
                ("m_DataSize".into(), V::UInt(8)),
                ("image_data".into(), V::Bytes(vec![])),
                (
                    "m_StreamData".into(),
                    V::Struct(
                        "StreamingInfo".into(),
                        vec![
                            ("path".into(), V::Str(b"array.resS".to_vec())),
                            ("offset".into(), V::UInt64(2)),
                            ("size".into(), V::UInt(8)),
                        ],
                    ),
                ),
            ],
        ),
    );
    let file = file_on(Some(13), &[], &[object], true).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let mut resource = vec![0, 0];
    resource.extend_from_slice(&data);
    env.load(Source::Bytes {
        name: RawStr::from("array.resS"),
        data: Bytes::from(resource),
    })
    .unwrap();
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("array.assets"),
            data: Bytes::from(file),
        })
        .unwrap()
    else {
        panic!("serialized");
    };
    let meter = Meter::unlimited();
    let images = array::images(&file.object(1).unwrap(), &env, &meter).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images.as_slice()[0].pixels(), &data[..4]);
    assert_eq!(images.as_slice()[1].pixels(), &data[4..]);
    drop(images);
    assert_eq!(meter.used(), 0);
}
