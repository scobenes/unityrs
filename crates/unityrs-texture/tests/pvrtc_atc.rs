//! Decoder contracts independent of the generated oracle fixture.
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_texture::{
    Format, Formats, Selection, Skip, TextureFields, decode, plan, select, select_in,
};

#[test]
fn six_formats_decode_small_images_and_join_only_explicit_batch_lists() {
    for value in [30, 31, 32, 33, 35, 36] {
        let f = Format::from_texture_format(value).expect("P3 T5 format");
        let data = vec![0; if value == 36 { 16 } else { 8 }];
        let p = plan(f, data.len(), 1, 1).unwrap();
        assert_eq!(p.input_bytes, u64::try_from(data.len()).unwrap());
        assert_eq!(p.scratch, 0);
        assert_eq!(p.output_bytes, 4);
        let meter = Meter::unlimited();
        let image = decode(f, &data, 1, 1, &meter).unwrap();
        assert_eq!(image.pixels(), [0, 0, 0, if value == 35 { 255 } else { 0 }]);
        assert_eq!(meter.used(), p.charge());
        drop(image);
        assert_eq!(meter.used(), 0);
        let fields = TextureFields {
            texture_format: value,
            width: 1,
            height: 1,
            platform: 19,
            platform_blob: None,
        };
        assert_eq!(
            select(&fields),
            Selection::Skip(Skip::UnsupportedFormat(value))
        );
        assert_eq!(
            select_in(&fields, Formats::DECODED, (0, 0)),
            Selection::Export(f)
        );
        assert_eq!(Formats::decoded(f.name()), Ok(f));
        for limit in [p.charge() - 1, p.charge()] {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            assert_eq!(decode(f, &data, 1, 1, &meter).is_ok(), limit == p.charge());
            assert_eq!(meter.used(), 0);
        }
        assert!(plan(f, data.len() - 1, 1, 1).is_err());
        assert!(plan(f, 1, -1, 1).is_err());
        assert_eq!(plan(f, 0, 0, -1).unwrap().output_bytes, 0);
    }
}

#[test]
fn pvrtc_rejects_non_power_of_two_block_grids_before_reading_data() {
    for (value, bw) in [(30, 8), (31, 8), (32, 4), (33, 4)] {
        let f = Format::from_texture_format(value).unwrap();
        for (w, h) in [(3 * bw, 8), (bw, 12), (3 * bw - 1, 7)] {
            let err = plan(f, 1, w, h).unwrap_err();
            assert_eq!(err.to_string(), "PVRTC block counts must be powers of two");
            assert_eq!(decode(f, &[0], w, h, &Meter::unlimited()).unwrap_err(), err);
        }
    }
}

#[test]
fn atc_rgba_uses_interpolated_alpha_and_colour_subtraction_clamps() {
    let f = Format::from_texture_format(36).unwrap();
    // Alpha index 7 everywhere: 255 for endpoints a0 <= a1; 17 for a0 > a1.
    for (a0, a1, want) in [(0, 0, 255), (120, 0, 17)] {
        let mut data = [0xff; 16];
        data[0] = a0;
        data[1] = a1;
        // High-bit mode, endpoint zero, endpoint one white, index 1: clamps to black.
        data[8..].copy_from_slice(&[0, 128, 255, 255, 85, 85, 85, 85]);
        let image = decode(f, &data, 3, 3, &Meter::unlimited()).unwrap();
        assert_eq!(image.pixels(), [0, 0, 0, want].repeat(9));
    }
}

#[test]
fn pvrtc_rgb_names_keep_alpha_and_punch_through_keeps_colour() {
    for value in [32, 33] {
        let f = Format::from_texture_format(value).unwrap();
        // A white, B black, both opaque; all modulation codes 2 in punch mode.
        let block = [0xaa, 0xaa, 0xaa, 0xaa, 0xff, 0xff, 0, 0x80];
        let image = decode(f, &block, 3, 3, &Meter::unlimited()).unwrap();
        assert_eq!(image.pixels(), [127, 127, 127, 0].repeat(9));
        // RGB and RGBA both use the encoded alpha (zero here).
        let image = decode(f, &[0; 8], 1, 1, &Meter::unlimited()).unwrap();
        assert_eq!(image.pixels(), [0, 0, 0, 0]);
    }
    for value in [30, 31] {
        let f = Format::from_texture_format(value).unwrap();
        // Every explicit weight is B (black); each missing checkerboard
        // weight must wrap onto explicit samples, including a 1x1 block grid.
        for bits in [0xffff_fffe_u32, 0xffef_ffff, 0xffff_ffff] {
            let mut data = bits.to_le_bytes().to_vec();
            data.extend_from_slice(&[0xff, 0xff, 0, 0x80]);
            let image = decode(f, &data, 7, 3, &Meter::unlimited()).unwrap();
            assert_eq!(image.pixels(), [0, 0, 0, 255].repeat(21));
        }
    }
}

#[test]
fn pvrtc_morton_order_and_edge_wrap_interpolate_the_opposite_edge() {
    // A four-block horizontal grid: red, green, blue, white, encoded in
    // Morton order (the long-axis tail is linear when ny=1). A=B in each
    // block, so modulation is irrelevant. Each block center is its colour.
    for value in [30, 31, 32, 33] {
        let f = Format::from_texture_format(value).unwrap();
        let bw = if value < 32 { 8 } else { 4 };
        let mut data = Vec::new();
        for c in [0xfc00_u16, 0x83e0, 0x801f, 0xffff] {
            data.extend_from_slice(&[0; 4]);
            // A's blue has only four stored bits; clear modulation mode.
            data.extend_from_slice(&(c & !1).to_le_bytes());
            data.extend_from_slice(&c.to_le_bytes());
        }
        let image = decode(f, &data, bw * 4, 4, &Meter::unlimited()).unwrap();
        // x=0 blends block 3 white with block 0 red, wrapping left.
        assert_eq!(&image.pixels()[..4], &[255, 127, 127, 255]);
        for (i, colour) in [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ]
        .iter()
        .enumerate()
        {
            let x = i * usize::try_from(bw).unwrap() + usize::try_from(bw).unwrap() / 2;
            assert_eq!(&image.pixels()[x * 4..x * 4 + 4], colour);
        }
    }
}

#[test]
fn atc_plans_have_no_padding_crop_and_pvrtc_checks_its_grid_before_pillow() {
    use unityrs_texture::DecodeError;
    for value in [35, 36] {
        let f = Format::from_texture_format(value).unwrap();
        let bytes = if value == 35 { 8 } else { 16 };
        assert_eq!(f.block_size(), None);
        let p = plan(f, 15 * 758_293 * bytes, 59, 3_033_169).unwrap();
        assert_eq!(p.output_bytes, 59 * 3_033_169 * 4);
        assert_eq!(p.scratch, 0);
        assert_eq!(
            plan(f, 1, 67_108_857, 1),
            Err(DecodeError::PillowMemoryError {
                width: 67_108_857,
                max: 67_108_856
            })
        );
    }
    for value in [30, 31, 32, 33] {
        let f = Format::from_texture_format(value).unwrap();
        assert_eq!(plan(f, 1, 67_108_856, 1), Err(DecodeError::PvrtcBlockCount));
        assert_eq!(
            plan(f, 1, 67_108_864, 1),
            Err(DecodeError::PillowMemoryError {
                width: 67_108_864,
                max: 67_108_856
            })
        );
    }
}
