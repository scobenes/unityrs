//! The bound UnityPy does not have (`MAX_PIXELS`), checked before the data;
//! the meter's charge and its refusal; Pillow's `BCn` length rule; and no
//! panic on any input, `plan()` giving the decode's answer. The limits
//! Pillow has below the bound (its raw decoder's width cap, its crop's
//! decompression-bomb check) are the decode module's unit tests.

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter, heap_cost};
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, Image, MAX_PIXELS, Mode, Plan, Selection, Skip, SkippedPlatform,
    TextureFields, bcn_length, decode, plan, select,
};

/// Data enough for any texture within the bound: 1 GiB.
const ENOUGH: usize = 1 << 30;

#[test]
fn the_largest_texture_plans_and_one_more_column_is_refused() {
    let side = 16384;
    for format in Format::ALL {
        let p = plan(format, ENOUGH, side, side).unwrap();
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
        // A block format pads the extra column to a whole block.
        let columns = if format.block_bytes().is_some() {
            side + 4
        } else {
            side + 1
        };
        let too_large = Err(DecodeError::TooLarge {
            pixels: u64::try_from(columns).unwrap() * u64::try_from(side).unwrap(),
            max: MAX_PIXELS,
        });
        // Whatever the data: enough, short, a byte.
        for len in [ENOUGH, 1 << 20, 1] {
            assert_eq!(plan(format, len, side + 1, side), too_large, "{format:?}");
        }
    }
    // A block format's padding counts: 16383 pads to 16384, within the
    // bound, but UnityPy's crop back to 16383 passes Pillow's
    // decompression-bomb bound.
    assert!(matches!(
        plan(Format::Bc7, ENOUGH, 16383, 16384),
        Err(DecodeError::DecompressionBomb { .. })
    ));
}

#[test]
fn sizes_do_not_wrap() {
    for format in Format::ALL {
        for (w, h) in [(i32::MAX, i32::MAX), (i32::MAX, 1), (1, i32::MAX)] {
            // Past the bound, whatever the data, and nothing charged.
            assert!(
                matches!(
                    plan(format, ENOUGH, w, h),
                    Err(DecodeError::TooLarge { .. })
                ),
                "{format:?} {w}x{h}"
            );
            let meter = Meter::unlimited();
            let got = decode(format, &[1, 2, 3], w, h, &meter);
            assert!(matches!(got, Err(DecodeError::TooLarge { .. })), "{got:?}");
            assert_eq!(meter.used(), 0);
        }
        for (w, h) in [
            (i32::MIN, 4),
            (4, i32::MIN),
            (i32::MIN, i32::MIN),
            (-1, i32::MAX),
        ] {
            assert_eq!(
                plan(format, ENOUGH, w, h),
                Err(DecodeError::NegativeSize {
                    width: w,
                    height: h
                })
            );
        }
        // A side of 0 is the empty RGBA image, whatever the other side and
        // the data.
        for (w, h) in [(0, i32::MIN), (i32::MAX, 0), (0, 0)] {
            for len in [0, 1, ENOUGH] {
                assert_eq!(
                    plan(format, len, w, h),
                    Ok(Plan {
                        mode: Mode::Rgba,
                        width: 0,
                        height: 0,
                        input_bytes: 0,
                        output_bytes: 0
                    })
                );
            }
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
        let p = plan(format, data.len(), 64, 64).unwrap();
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

/// A real decode of data past 4 GiB: a 4x4 BC7 texture from 2^32 + 16
/// zeroed bytes (calloc'd, so only the pages read are touched), whose
/// length Pillow reads as 16, decodes as its first 16 bytes do.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_decode_from_more_than_4_gib_reads_the_low_32_bits() {
    let data = vec![0u8; (1 << 32) + 16];
    let meter = Meter::unlimited();
    let image = decode(Format::Bc7, &data, 4, 4, &meter).unwrap();
    let alone = decode(Format::Bc7, &data[..16], 4, 4, &meter).unwrap();
    assert_eq!(image.pixels(), alone.pixels());
    // Pillow's degenerate BC7 block: opaque black.
    assert_eq!(image.pixels(), [0, 0, 0, 255].repeat(16));
    drop((image, alone));
    assert_eq!(meter.used(), 0);
}

/// The sides the seeded test draws from besides small ones: the ends of
/// `i32` and the small ones, the bound's (16384, and 2^26 to 2^28 as one
/// side), Pillow's raw width caps and its crop bound's (the fewest pixels
/// past it, 59 x 3,033,169, and the bound itself, 12,470 x 14,351).
const EDGES: [i32; 44] = [
    i32::MIN,
    i32::MIN + 1,
    -65536,
    -8,
    -7,
    -6,
    -5,
    -4,
    -3,
    -2,
    -1,
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    16380,
    16381,
    16382,
    16383,
    16384,
    16385,
    16386,
    16387,
    16388,
    1 << 26,
    1 << 27,
    1 << 28,
    67_108_856,
    67_108_857,
    89_478_478,
    89_478_479,
    59,
    3_033_169,
    12_470,
    14_351,
    i32::MAX - 3,
    i32::MAX - 2,
    i32::MAX - 1,
    i32::MAX,
];

/// Past this, a round's data is zeroed (calloc'd, untouched unless read)
/// and a decode the plan allows is refused by its meter, so the rounds stay
/// fast.
const BIG: u64 = 1 << 20;

/// Any format, size, data length and meter: an image or a named error,
/// never a panic; the plan's answer is the decode's; and the meter is back
/// at 0 once the image drops (`docs/plans/core.md` section 3.15's 10,000
/// cases).
#[test]
fn no_panic_on_any_input() {
    let mut rng = Rng::new(0x7e57_0000_0000_0001);
    let mut decoded = 0;
    for round in 0..10_000 {
        let format = Format::ALL[rng.below(5)];
        let side = |rng: &mut Rng| {
            if rng.below(2) == 0 {
                EDGES[rng.below(EDGES.len())]
            } else {
                i32::try_from(rng.below(48)).unwrap() - 4
            }
        };
        let (w, h) = (side(&mut rng), side(&mut rng));
        // What mip 0 needs, where the checks get as far as the data.
        let need = match plan(format, 1, w, h) {
            Err(DecodeError::NotEnoughData { needed, .. }) => needed,
            Ok(p) => p.input_bytes,
            Err(_) => 64,
        };
        let len = match rng.below(5) {
            0 => 0,
            1 => need.saturating_sub(1),
            2 => need,
            3 => need + 1,
            _ => {
                let most = usize::try_from(need.min(BIG)).unwrap() * 2 + 17;
                u64::try_from(rng.below(most)).unwrap()
            }
        };
        let len = usize::try_from(len).unwrap();
        let mut data = vec![0u8; len];
        if need <= BIG {
            for chunk in data.chunks_mut(8) {
                let bytes = rng.next_u64().to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
        let planned = plan(format, len, w, h);
        let charge = planned.as_ref().map_or(0, Plan::charge);
        let big = planned.as_ref().is_ok_and(|p| p.output_bytes > BIG);
        // A quarter, and every big one, under a meter of about the charge.
        let limit = if big {
            Some(charge - 1)
        } else if rng.below(4) == 0 {
            Some((charge + u64::try_from(rng.below(3)).unwrap()).saturating_sub(1))
        } else {
            None
        };
        let meter = limit.map_or_else(Meter::unlimited, |l| {
            Meter::new(l, Arc::new(Unbounded), Reservation::Input)
        });
        let what = format!("round {round}: {format:?} {w}x{h}, {len} bytes, limit {limit:?}");
        match (decode(format, &data, w, h, &meter), planned) {
            (Ok(image), Ok(p)) => {
                decoded += 1;
                check(&image, &p, &what);
                assert_eq!(meter.used(), image.charge().bytes(), "{what}");
            }
            (Err(DecodeError::Panicked { payload }), _) => panic!("{what}: {payload}"),
            (Err(DecodeError::Charge(ChargeError::Limit(_))), Ok(_)) => {
                assert!(limit.is_some_and(|l| l < charge), "{what}");
            }
            (got, planned) => assert_eq!(got.map(|_| ()), planned.map(|_| ()), "{what}"),
        }
        assert_eq!(meter.used(), 0, "{what}");
    }
    assert!(decoded > 1000, "{decoded} decodes");
}

fn check(image: &Image, p: &Plan, what: &str) {
    assert_eq!(
        u64::try_from(image.pixels().len()).unwrap(),
        p.output_bytes,
        "{what}"
    );
    assert_eq!(
        (image.width(), image.height(), image.mode()),
        (p.width, p.height, p.mode),
        "{what}"
    );
    assert_eq!(image.charge().bytes(), p.charge(), "{what}");
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
