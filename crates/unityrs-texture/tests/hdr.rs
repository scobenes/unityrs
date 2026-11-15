use std::sync::Arc;

use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{ChargeError, Meter},
};
use unityrs_texture::{
    DecodeError, Format, Formats, HdrError, SWITCH, Selection, Skip, SkippedPlatform,
    TextureFields, XBOX360, array, decode, decode_bc6h_signed, decode_in, plan, plan_in, select,
    select_in,
};

#[test]
fn hdr_formats_decode_half_colours_with_the_shared_meter_and_gates() {
    for (value, side) in [(66, 4), (67, 5), (68, 6), (69, 8), (70, 10), (71, 12)] {
        let format = Format::from_texture_format(value).expect("HDR format registered");
        assert!(Formats::DECODED.contains(format));
        assert!(!Formats::V1.contains(format));
        // FP16 void extent, unbounded extent, exact half values.
        let mut block = [255; 16];
        block[..2].copy_from_slice(&[0xfc, 0xff]);
        for (dst, value) in block[8..]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip([0x3800u16, 0x3400, 0x3c00, 0x3800])
        {
            *dst = value.to_le_bytes();
        }
        let p = plan(format, 16, side - 1, side - 2).unwrap();
        let ldr = Format::from_texture_format(value - 12).unwrap();
        assert_eq!(
            p.scratch,
            plan(ldr, 16, side - 1, side - 2).unwrap().scratch
        );
        let meter = Meter::unlimited();
        let image = decode(format, &block, side - 1, side - 2, &meter).unwrap();
        assert_eq!(image.mode().name(), "RGBA");
        assert!(
            image
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == [128, 64, 255, 128])
        );
        assert_eq!(meter.used(), p.charge());
        drop(image);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn bc6h_is_rgb_and_participates_only_in_explicit_format_lists() {
    let format = Format::from_texture_format(24).expect("BC6H registered");
    assert_eq!(format.mode().name(), "RGB");
    assert_eq!(format.block_size(), Some((4, 4)));
    assert!(!Formats::V1.contains(format));
    let image = decode(format, &[0; 16], 3, 2, &Meter::unlimited()).unwrap();
    assert_eq!(image.pixels(), &[0; 18]);
    assert_eq!(plan(format, 16, 3, 2).unwrap().scratch, 0);
}

#[test]
fn illegal_hdr_content_returns_its_typed_error_and_releases_every_charge() {
    let mut invalid = [0; 16];
    invalid[0] = 19; // a reserved BC6H mode
    for (value, bytes, error) in [
        (24, invalid, HdrError::ReservedMode),
        (66, [0; 16], HdrError::InvalidBlock),
        (71, [0; 16], HdrError::InvalidBlock),
    ] {
        let format = Format::from_texture_format(value).unwrap();
        let meter = Meter::unlimited();
        assert!(plan(format, bytes.len(), 1, 1).is_ok());
        assert_eq!(
            decode(format, &bytes, 1, 1, &meter).err(),
            Some(DecodeError::Hdr(error))
        );
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn explicit_hdr_selection_obeys_the_existing_platform_and_empty_image_gates() {
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    for texture_format in [24, 66, 67, 68, 69, 70, 71] {
        let mut fields = TextureFields {
            texture_format,
            width: 4,
            height: 4,
            platform: 19,
            platform_blob: None,
        };
        let format = Format::from_texture_format(texture_format).unwrap();
        assert_eq!(
            select_in(&fields, Formats::DECODED, (2021, 3)),
            Selection::Export(format)
        );
        assert_eq!(
            select(&fields),
            Selection::Skip(Skip::UnsupportedFormat(texture_format))
        );
        fields.platform = SWITCH;
        fields.platform_blob = Some(&blob);
        assert_eq!(
            select_in(&fields, Formats::DECODED, (2021, 3)),
            if texture_format == 24 {
                Selection::Export(format)
            } else {
                Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
            }
        );
        fields.width = 0;
        assert_eq!(
            select_in(&fields, Formats::DECODED, (2021, 3)),
            Selection::Skip(Skip::EmptyImage)
        );
    }
}

fn bc6h_colour(red: u128, green: u128, blue: u128) -> [u8; 16] {
    // Mode 3, zero indices: saturated base channels, zero second endpoint.
    (3u128 | red << 5 | green << 15 | blue << 25).to_le_bytes()
}

#[test]
fn switch_bc6h_deswizzles_before_decoding_then_crops_before_flipping() {
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    let fields = TextureFields {
        texture_format: 24,
        width: 5,
        height: 5,
        platform: SWITCH,
        platform_blob: Some(&blob),
    };
    let mut data = vec![0; 1024];
    // The first GOB stores blocks (0,0), (0,1), (1,0), ... .
    data[..16].copy_from_slice(&bc6h_colour(1023, 0, 0));
    data[16..32].copy_from_slice(&bc6h_colour(0, 1023, 0));
    data[32..48].copy_from_slice(&bc6h_colour(0, 0, 1023));
    let p = plan_in(&fields, data.len()).unwrap();
    let meter = Meter::new(p.peak_charge(), Arc::new(Unbounded), Reservation::Input);
    let image = decode_in(&fields, &data, &meter).unwrap();
    let mut expected = [0, 255, 0].repeat(4);
    expected.extend_from_slice(&[0, 0, 0]);
    for _ in 0..4 {
        expected.extend_from_slice(&[255, 0, 0].repeat(4));
        expected.extend_from_slice(&[0, 0, 255]);
    }
    assert_eq!((image.width(), image.height()), (5, 5));
    assert_eq!(image.pixels(), expected);
    assert_eq!(meter.used(), p.charge());
    assert_eq!(meter.decoded(), 1024 * 2 + 16 * 64 * 3 + 5 * 5 * 3);
    drop(image);
    assert_eq!(meter.used(), 0);
}

#[test]
fn bc6h_array_slices_keep_mip_tails_and_resource_fallback_with_bounded_charges() {
    let fields = array::ArrayFields {
        graphics_format: 106,
        width: 3,
        height: 2,
        depth: 2,
        data_size: 64,
        platform: 13,
    };
    let mut data = Vec::new();
    for block in [bc6h_colour(1023, 0, 0), bc6h_colour(0, 0, 1023)] {
        data.extend_from_slice(&block);
        // Reserved-mode mip tails must stay outside each slice's mip zero.
        data.extend_from_slice(&[19; 16]);
    }
    let p = array::plan(&fields, data.len()).unwrap();
    assert_eq!((p.slice_count, p.slice_bytes), (2, 32));
    assert_eq!(p.image.mode.name(), "RGB");
    let meter = Meter::new(p.peak_charge, Arc::new(Unbounded), Reservation::Input);
    let images = array::decode(&fields, &[], Some(&data), &meter).unwrap();
    assert_eq!(images.as_slice()[0].pixels(), [255, 0, 0].repeat(6));
    assert_eq!(images.as_slice()[1].pixels(), [0, 0, 255].repeat(6));
    assert_eq!(meter.used(), p.peak_charge);
    assert_eq!(meter.decoded(), 2 + 2 * 3 * 2 * 3);
    drop(images);
    assert_eq!(meter.used(), 0);
    for platform in [SWITCH, XBOX360] {
        let fields = array::ArrayFields { platform, ..fields };
        assert_eq!(
            array::decode(&fields, &data, None, &meter).err(),
            Some(array::ArrayError::UnsupportedPlatform(platform))
        );
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn signed_unsigned_and_astc_hdr_count_output_work_once_before_allocating() {
    let bc6h = [0; 16];
    let mut astc = [255; 16];
    astc[0] = 0xfc;
    for (format, data, signed) in [
        (Format::Bc6h, bc6h, false),
        (Format::Bc6h, bc6h, true),
        (Format::AstcHdr4x4, astc, false),
    ] {
        let p = plan(format, data.len(), 3, 2).unwrap();
        for work in [p.output_bytes - 1, p.output_bytes] {
            let meter = Meter::with_decode_work(
                p.peak_charge(),
                work,
                Arc::new(Unbounded),
                Reservation::Input,
            );
            let result = if signed {
                decode_bc6h_signed(&data, 3, 2, &meter)
            } else {
                decode(format, &data, 3, 2, &meter)
            };
            if work < p.output_bytes {
                assert!(matches!(
                    result,
                    Err(DecodeError::Charge(ChargeError::Limit(_)))
                ));
                assert_eq!(meter.decoded(), 0);
            } else {
                assert!(result.is_ok(), "{format:?}, signed={signed}: {result:?}");
                assert_eq!(meter.used(), p.charge());
                assert_eq!(meter.decoded(), p.output_bytes);
            }
            drop(result);
            assert_eq!(meter.used(), 0);
        }
    }
}
