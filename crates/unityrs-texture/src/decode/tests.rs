use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::limits::Meter;

use super::{DecodeError, Format, MAX_CROP_PIXELS, MAX_PIXELS, decode, decode_with, plan};

/// The bound comes before the data (step 4): a texture past [`MAX_PIXELS`]
/// is too large whatever its data, but empty data or a negative side is
/// named first.
#[test]
fn the_bound_comes_before_the_data() {
    let (w, h) = (16388, 16384);
    let needed = 4097 * 4096 * 8;
    let too_large = Err(DecodeError::TooLarge {
        pixels: 16388 * 16384,
        max: MAX_PIXELS,
    });
    for len in [1, needed - 1, needed, needed + 1] {
        assert_eq!(plan(Format::Dxt1, len, w, h), too_large, "{len}");
    }
    assert_eq!(plan(Format::Dxt1, 0, w, h), Err(DecodeError::NoImageData));
    assert_eq!(
        plan(Format::Dxt1, 1, -w, h),
        Err(DecodeError::NegativeSize {
            width: -w,
            height: h
        })
    );
    assert!(matches!(
        plan(Format::Rgba32, 3, i32::MAX, i32::MAX),
        Err(DecodeError::TooLarge { .. })
    ));
    // decode() gives the plan's answer, with nothing charged.
    let meter = Meter::unlimited();
    assert_eq!(
        decode(Format::Dxt1, &[1, 2, 3], w, h, &meter).map(|_| ()),
        too_large.map(|_| ())
    );
    assert_eq!(meter.used(), 0);
}

/// Pillow's raw decoder refuses a row of more than `INT_MAX / bits - 7`
/// pixels before it reads the data (step 5); the `bcn` decoder has no cap.
#[test]
fn pillows_raw_width_cap_comes_before_the_data() {
    for (format, max) in [(Format::Rgba32, 67_108_856), (Format::Rgb24, 89_478_478)] {
        let bpp = format.mode().bytes_per_pixel();
        assert_eq!(
            plan(format, 1, max, 1),
            Err(DecodeError::NotEnoughData {
                needed: u64::from(max.unsigned_abs()) * bpp,
                got: 1
            }),
            "{format:?}"
        );
        let need = usize::try_from(u64::from(max.unsigned_abs()) * bpp).unwrap();
        assert!(plan(format, need, max, 1).is_ok(), "{format:?}");
        let refused = Err(DecodeError::PillowMemoryError {
            width: max.unsigned_abs() + 1,
            max: max.unsigned_abs(),
        });
        for len in [1, need + usize::try_from(bpp).unwrap()] {
            assert_eq!(plan(format, len, max + 1, 1), refused, "{format:?}");
        }
        assert_eq!(plan(format, 1, max + 1, 2), refused, "{format:?}");
        let meter = Meter::unlimited();
        assert_eq!(
            decode(format, &[0], max + 1, 1, &meter).map(|_| ()),
            refused.map(|_| ())
        );
        assert_eq!(meter.used(), 0);
    }
    // 67,108,860 x 4 padded pixels: under the bound, and not cropped past
    // Pillow's crop bound either.
    let bc7 = 67_108_860 / 4 * 16;
    assert!(plan(Format::Bc7, bc7, 67_108_860, 1).is_ok());
}

/// UnityPy's crop of a padded `BCn` image raises Pillow's
/// `DecompressionBombError` past [`MAX_CROP_PIXELS`] (step 8), after the
/// data check; an aligned texture and a raw one are not cropped.
#[test]
fn the_crop_bomb_comes_after_the_data() {
    // 59 x 3,033,169 is the fewest pixels past the crop bound: 178,956,971.
    let (w, h) = (59, 3_033_169);
    let need = 15 * 758_293 * 8;
    assert_eq!(
        plan(Format::Dxt1, need - 1, w, h),
        Err(DecodeError::NotEnoughData {
            needed: 90_995_160,
            got: 90_995_159
        })
    );
    let bomb = plan(Format::Dxt1, need, w, h).unwrap_err();
    assert_eq!(
        bomb,
        DecodeError::DecompressionBomb {
            pixels: 178_956_971,
            max: MAX_CROP_PIXELS
        }
    );
    assert_eq!(
        bomb.to_string(),
        "Image size (178956971 pixels) exceeds limit of 178956970 pixels, could be \
         decompression bomb DOS attack."
    );
    for format in [Format::Dxt5, Format::Bc7] {
        assert!(matches!(
            plan(format, 2 * need, w, h),
            Err(DecodeError::DecompressionBomb { .. })
        ));
    }
    // 12,470 x 14,351 is exactly the bound, cropped from 12,472 x 14,352.
    assert!(plan(Format::Dxt1, 12_472 / 4 * 14_352 / 4 * 8, 12_470, 14_351).is_ok());
    // 4 x 44,739,244 is past it, but aligned: not cropped.
    assert!(plan(Format::Dxt1, 44_739_244 / 4 * 8, 4, 44_739_244).is_ok());
    // A raw texture is not cropped.
    assert!(plan(Format::Rgba32, 4 * 178_956_971, w, h).is_ok());
    // A block format's padding counts toward the bound, not the crop's.
    assert!(matches!(
        plan(Format::Bc7, 1 << 28, 16383, 16384),
        Err(DecodeError::DecompressionBomb { .. })
    ));
    assert!(plan(Format::Bc7, 1 << 28, 16384, 16384).is_ok());
}

/// Pillow's `BCn` decoder sees the data's length as a C `int`.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_bcn_length_past_2_gib_is_read_as_its_low_32_bits() {
    // A 4x4 BC7 texture needs 16 bytes.
    assert!(plan(Format::Bc7, (1 << 32) + 16, 4, 4).is_ok());
    assert_eq!(
        plan(Format::Bc7, (1 << 32) + 15, 4, 4),
        Err(DecodeError::NotEnoughData {
            needed: 16,
            got: (1 << 32) + 15
        })
    );
    assert!(matches!(
        plan(Format::Bc7, (1 << 31) + 16, 4, 4),
        Err(DecodeError::NotEnoughData { .. })
    ));
    // The raw formats read the whole length.
    assert!(plan(Format::Rgba32, (1 << 31) + 16, 2, 2).is_ok());
}

/// A panic in the pixel loop is the decode's [`DecodeError::Panicked`], and
/// the unwinding frees the output and returns its charge.
#[test]
fn a_panic_in_the_pixel_loop_is_contained() {
    let meter = Meter::unlimited();
    let data = vec![0u8; 4 * 64 * 64];
    let got = decode_with(Format::Rgba32, &data, 64, 64, &meter, |_, _, _, _, out| {
        assert_eq!(out.len(), 4 * 64 * 64);
        panic!("in the loop");
    });
    match got {
        Err(DecodeError::Panicked { payload }) => assert_eq!(payload.message(), "in the loop"),
        other => panic!("{other:?}"),
    }
    assert_eq!(meter.used(), 0);
    // The checks run outside it: their errors are their own.
    let got = decode_with(Format::Rgba32, &[], 64, 64, &meter, |_, _, _, _, _| {
        panic!("not reached")
    });
    assert_eq!(got.map(|_| ()), Err(DecodeError::NoImageData));
}

/// A budget that panics when the meter asks it.
#[derive(Debug)]
struct Panicking;

impl MemoryBudget for Panicking {
    fn try_reserve(&self, _: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        panic!("the budget panicked");
    }
}

/// The output's charge runs outside the decoder's containment: a budget
/// that panics unwinds out of `decode()`, and is never a
/// [`DecodeError::Panicked`], which would be taken for the decoder's. (The
/// meter keeps the bytes it counted before it asked: the core's matter.)
#[test]
fn a_panicking_budget_unwinds_out_of_the_decode() {
    let meter = Meter::new(u64::MAX, Arc::new(Panicking), Reservation::Input);
    let data = vec![0u8; 4 * 4 * 4];
    let got = catch_unwind(AssertUnwindSafe(|| {
        decode(Format::Rgba32, &data, 4, 4, &meter).map(|_| ())
    }));
    match got {
        Err(payload) => assert_eq!(payload.downcast_ref::<&str>(), Some(&"the budget panicked")),
        Ok(answer) => panic!("decode() answered {answer:?}"),
    }
}
