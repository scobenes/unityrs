//! Corrected ASTC HDR array mapping, independently specified FP16 colours,
//! slice mip tails, crop/flip ordering and charge lifetimes.
use std::sync::Arc;

use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{ChargeError, Meter},
};
use unityrs_texture::{
    DecodeError, HdrError, SWITCH, XBOX360,
    array::{self, ArrayError, ArrayFields},
};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255; 4];

fn void_block(colour: [u8; 4]) -> [u8; 16] {
    let mut block = [255; 16];
    block[0] = 0xfc;
    for (channel, bytes) in colour.into_iter().zip(block[8..].as_chunks_mut::<2>().0) {
        let half = if channel == 0 { 0u16 } else { 0x3c00 };
        *bytes = half.to_le_bytes();
    }
    block
}

fn fields(graphics_format: i32, side: i32) -> ArrayFields {
    ArrayFields {
        graphics_format,
        width: side + 1,
        height: side + 2,
        depth: 2,
        data_size: 160,
        platform: 13,
    }
}

fn data() -> Vec<u8> {
    let mut data = Vec::new();
    for colours in [[RED, GREEN, BLUE, WHITE], [BLUE, WHITE, RED, GREEN]] {
        for colour in colours {
            data.extend_from_slice(&void_block(colour));
        }
        // An illegal block in the mip tail must never reach the decoder.
        data.extend_from_slice(&[0; 16]);
    }
    data
}

fn expected(side: usize, bottom: [u8; 4], right: [u8; 4], top: [u8; 4], end: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::new();
    // A side+1 by side+2 crop keeps two rows of the lower block row.
    // Vertical flipping places those rows first.
    for _ in 0..2 {
        pixels.extend_from_slice(&bottom.repeat(side));
        pixels.extend_from_slice(&right);
    }
    for _ in 0..side {
        pixels.extend_from_slice(&top.repeat(side));
        pixels.extend_from_slice(&end);
    }
    pixels
}

#[test]
fn all_six_hdr_array_footprints_decode_slices_with_mip_tails_crop_flip_and_resource_fallback() {
    let data = data();
    for (graphics, side) in [(145, 4), (146, 5), (147, 6), (148, 8), (149, 10), (150, 12)] {
        let fields = fields(graphics, side);
        let plan = array::plan(&fields, data.len()).unwrap();
        assert_eq!((plan.slice_count, plan.slice_bytes), (2, 80));
        assert_eq!(plan.image.input_bytes, 64);
        assert_eq!(plan.image.mode.name(), "RGBA");
        for streamed in [false, true] {
            let meter = Meter::new(plan.peak_charge, Arc::new(Unbounded), Reservation::Input);
            let (inline, resource) = if streamed {
                (&[][..], Some(data.as_slice()))
            } else {
                // A present inline buffer wins over a bad resource buffer.
                (data.as_slice(), Some(&[0; 160][..]))
            };
            let images = array::decode(&fields, inline, resource, &meter).unwrap();
            assert_eq!(images.len(), 2);
            let side = usize::try_from(side).unwrap();
            assert_eq!(
                images.as_slice()[0].pixels(),
                expected(side, BLUE, WHITE, RED, GREEN),
                "graphics {graphics}, streamed={streamed}"
            );
            assert_eq!(
                images.as_slice()[1].pixels(),
                expected(side, RED, GREEN, BLUE, WHITE),
                "graphics {graphics}, streamed={streamed}"
            );
            assert_eq!(meter.used(), plan.peak_charge - plan.image.scratch);
            assert_eq!(meter.decoded(), 2 + 2 * plan.image.output_bytes);
            drop(images);
            assert_eq!(meter.used(), 0);
        }
        let meter = Meter::new(
            plan.peak_charge - 1,
            Arc::new(Unbounded),
            Reservation::Input,
        );
        assert!(matches!(
            array::decode(&fields, &data, None, &meter),
            Err(ArrayError::Decode(DecodeError::Charge(ChargeError::Limit(
                _
            ))))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn an_illegal_second_hdr_slice_releases_prior_images_tables_and_output_charges() {
    let mut data = data();
    data[80..96].fill(0);
    for (graphics, side) in [(145, 4), (146, 5), (147, 6), (148, 8), (149, 10), (150, 12)] {
        let fields = fields(graphics, side);
        let plan = array::plan(&fields, data.len()).unwrap();
        let meter = Meter::new(plan.peak_charge, Arc::new(Unbounded), Reservation::Input);
        assert_eq!(
            array::decode(&fields, &data, None, &meter).err(),
            Some(ArrayError::Decode(DecodeError::Hdr(HdrError::InvalidBlock)))
        );
        assert_eq!(meter.used(), 0, "graphics {graphics}");
    }
}

#[test]
fn hdr_array_platform_gates_precede_mapping_or_allocating_slices() {
    let data = data();
    for graphics in 145..=150 {
        for platform in [SWITCH, XBOX360] {
            let fields = ArrayFields {
                platform,
                ..fields(graphics, 4)
            };
            let meter = Meter::unlimited();
            assert_eq!(
                array::plan(&fields, data.len()).err(),
                Some(ArrayError::UnsupportedPlatform(platform))
            );
            assert_eq!(
                array::decode(&fields, &data, None, &meter).err(),
                Some(ArrayError::UnsupportedPlatform(platform))
            );
            assert_eq!((meter.used(), meter.decoded()), (0, 0));
        }
    }
}

#[test]
fn corrected_hdr_mapping_keeps_the_oracle_map_and_other_unmapped_formats_unchanged() {
    let data = data();
    for graphics in 145..=150 {
        assert_eq!(
            array::unitypy_texture_format(graphics),
            Err(ArrayError::UnmappedGraphicsFormat(graphics))
        );
    }
    for graphics in [0, 107, 151] {
        let fields = fields(graphics, 4);
        assert_eq!(
            array::plan(&fields, data.len()).err(),
            Some(ArrayError::UnmappedGraphicsFormat(graphics))
        );
    }
    assert_eq!(
        array::plan(&fields(999, 4), data.len()).err(),
        Some(ArrayError::UnknownGraphicsFormat(999))
    );
}
