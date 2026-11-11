//! The bounds UnityPy does not have (`MAX_PIXELS`), the meter's charge and
//! its refusal, Pillow's `BCn` length rule, and no panic on any input.

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter, heap_cost};
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, MAX_PIXELS, Mode, Plan, Selection, Skip, SkippedPlatform, TextureFields,
    bcn_length, decode, plan, select,
};

#[test]
fn the_largest_texture_plans_and_one_more_column_is_refused() {
    let side = 16384;
    for format in Format::ALL {
        let p = plan(format, side, side).unwrap();
        assert_eq!(u64::from(p.width) * u64::from(p.height), MAX_PIXELS);
        assert_eq!(p.output_bytes, MAX_PIXELS * format.mode().bytes_per_pixel());
        let want = match format {
            Format::Rgb24 => 3 * MAX_PIXELS,
            Format::Rgba32 => 4 * MAX_PIXELS,
            Format::Dxt1 => MAX_PIXELS / 2,
            Format::Dxt5 | Format::Bc7 => MAX_PIXELS,
        };
        assert_eq!(p.input_bytes, want, "{format:?}");
        assert_eq!(p.charge(), heap_cost(p.output_bytes));
        let wider = plan(format, side + 1, side).unwrap_err();
        // A block format pads the extra column to a whole block.
        let columns = if format.block_bytes().is_some() {
            side + 4
        } else {
            side + 1
        };
        assert_eq!(
            wider,
            DecodeError::TooLarge {
                pixels: u64::try_from(columns).unwrap() * u64::try_from(side).unwrap(),
                max: MAX_PIXELS
            }
        );
    }
    // A block format's padding counts: 16383 pads to 16384.
    assert!(plan(Format::Bc7, 16383, 16384).is_ok());
}

#[test]
fn sizes_do_not_wrap() {
    for format in Format::ALL {
        for (w, h) in [(i32::MAX, i32::MAX), (i32::MAX, 1), (1, i32::MAX)] {
            assert!(
                matches!(plan(format, w, h), Err(DecodeError::TooLarge { .. })),
                "{format:?} {w}x{h}"
            );
            // With data, the data checks come first, as UnityPy's do: three
            // bytes are short of any of these.
            let meter = Meter::unlimited();
            let got = decode(format, &[1, 2, 3], w, h, &meter);
            assert!(
                matches!(got, Err(DecodeError::NotEnoughData { got: 3, .. })),
                "{got:?}"
            );
            assert_eq!(meter.used(), 0);
        }
        for (w, h) in [
            (i32::MIN, 4),
            (4, i32::MIN),
            (i32::MIN, i32::MIN),
            (-1, i32::MAX),
        ] {
            assert_eq!(
                plan(format, w, h),
                Err(DecodeError::NegativeSize {
                    width: w,
                    height: h
                })
            );
        }
        // A side of 0 is the empty RGBA image, whatever the other side.
        for (w, h) in [(0, i32::MIN), (i32::MAX, 0), (0, 0)] {
            assert_eq!(
                plan(format, w, h),
                Ok(Plan {
                    mode: Mode::Rgba,
                    width: 0,
                    height: 0,
                    input_bytes: 0,
                    output_bytes: 0
                })
            );
            let image = decode(format, &[], w, h, &Meter::unlimited()).unwrap();
            assert_eq!(
                (image.width(), image.height(), image.mode()),
                (0, 0, Mode::Rgba)
            );
            assert_eq!(image.pixels(), []);
            assert_eq!(image.charge().bytes(), 0);
        }
    }
}

#[test]
fn the_output_is_charged_before_it_is_allocated() {
    let data = vec![7u8; 4 * 64 * 64];
    for format in Format::ALL {
        let p = plan(format, 64, 64).unwrap();
        // One byte short of the charge: refused, nothing allocated.
        let meter = Meter::new(p.charge() - 1, Arc::new(Unbounded), Reservation::Input);
        let got = decode(format, &data, 64, 64, &meter);
        assert!(
            matches!(got, Err(DecodeError::Charge(ChargeError::Limit(_)))),
            "{format:?}: {got:?}"
        );
        assert_eq!(meter.used(), 0);
        // Exactly the charge: decoded, held while the image lives.
        let meter = Meter::new(p.charge(), Arc::new(Unbounded), Reservation::Input);
        let image = decode(format, &data, 64, 64, &meter).unwrap();
        assert_eq!(meter.used(), p.charge());
        let (pixels, charge) = image.into_parts();
        assert_eq!(u64::try_from(pixels.len()).unwrap(), p.output_bytes);
        drop(pixels);
        assert_eq!(meter.used(), p.charge());
        drop(charge);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn the_bcn_length_is_pillows_c_int() {
    assert_eq!(bcn_length(0), 0);
    assert_eq!(bcn_length(16), 16);
    assert_eq!(bcn_length(0x7FFF_FFFF), 0x7FFF_FFFF);
    #[cfg(target_pointer_width = "64")]
    {
        // 2 GiB and up: the low 32 bits, read as a signed int.
        assert_eq!(bcn_length(0x8000_0000), 0);
        assert_eq!(bcn_length(0xFFFF_FFFF), 0);
        assert_eq!(bcn_length(0x1_0000_0000), 0);
        assert_eq!(bcn_length(0x1_0000_0010), 16);
        assert_eq!(bcn_length(0x1_7FFF_FFFF), 0x7FFF_FFFF);
    }
}

/// Any format, size and data: an image or a named error, never a panic, and
/// the meter back at 0 once the image drops.
#[test]
fn no_panic_on_any_input() {
    let mut rng = Rng::new(0x7e57_0000_0000_0001);
    for round in 0..3000 {
        let format = Format::ALL[rng.below(5)];
        let side = |rng: &mut Rng| i32::try_from(rng.below(48)).unwrap() - 4;
        let (w, h) = (side(&mut rng), side(&mut rng));
        let need = plan(format, w, h).map_or(64, |p| p.input_bytes);
        let len = rng.below(usize::try_from(need).unwrap() * 2 + 17);
        let mut data = vec![0u8; len];
        for chunk in data.chunks_mut(8) {
            let bytes = rng.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        let meter = Meter::unlimited();
        match decode(format, &data, w, h, &meter) {
            Ok(image) => {
                let p = plan(format, w, h).unwrap();
                assert_eq!(u64::try_from(image.pixels().len()).unwrap(), p.output_bytes);
                assert_eq!((image.width(), image.height()), (p.width, p.height));
            }
            Err(DecodeError::Panicked { payload }) => {
                panic!("round {round}: {format:?} {w}x{h}, {len} bytes: {payload}")
            }
            Err(_) => {}
        }
        assert_eq!(meter.used(), 0, "round {round}");
    }
}

#[test]
fn skips_follow_unitypy_order() {
    let fields = |texture_format, width, platform, platform_blob| TextureFields {
        texture_format,
        width,
        height: 4,
        platform,
        platform_blob,
    };
    let swizzled: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    // A side of 0 first, whatever the format and the platform.
    assert_eq!(
        select(&fields(1000, 0, 38, Some(swizzled))),
        Selection::Skip(Skip::EmptyImage)
    );
    // Then the format.
    assert_eq!(
        select(&fields(1000, 4, 38, Some(swizzled))),
        Selection::Skip(Skip::UnsupportedFormat(1000))
    );
    assert_eq!(
        Skip::UnsupportedFormat(1000).to_string(),
        "unsupported_format(1000)"
    );
    assert_eq!(
        Skip::UnsupportedFormat(34).to_string(),
        "unsupported_format(ETC_RGB4)"
    );
    // Then the platform.
    assert_eq!(
        select(&fields(25, 4, 38, Some(swizzled))),
        Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
    );
    assert_eq!(
        select(&fields(10, 4, 11, None)),
        Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Xbox360))
    );
    assert_eq!(
        select(&fields(25, 4, 11, None)),
        Selection::Export(Format::Bc7)
    );
    // A negative side is no skip: the decode names it.
    assert_eq!(
        select(&fields(4, -4, 19, None)),
        Selection::Export(Format::Rgba32)
    );
}
