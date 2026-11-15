use unityrs_core::limits::Meter;
use unityrs_texture::{
    DecodeError, Format, Formats, HdrError, SWITCH, Selection, Skip, SkippedPlatform,
    TextureFields, decode, plan, select, select_in,
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
            Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
        );
        fields.width = 0;
        assert_eq!(
            select_in(&fields, Formats::DECODED, (2021, 3)),
            Selection::Skip(Skip::EmptyImage)
        );
    }
}
