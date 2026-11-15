//! Hand-computed closed forms from PARITY.md sections 17.1, 19.1 and 19.2.
use unityrs_core::limits::Meter;
use unityrs_texture::{Format, decode};

fn pixel(value: i32, bytes: &[u8], expected: &[u8]) {
    let format =
        Format::from_texture_format(value).unwrap_or_else(|| panic!("missing format {value}"));
    let meter = Meter::unlimited();
    let image = decode(format, bytes, 1, 1, &meter).unwrap();
    assert_eq!(image.pixels(), expected, "{}", format.name());
    assert_eq!(meter.used(), image.charge().bytes());
    drop(image);
    assert_eq!(meter.used(), 0);
}

#[test]
fn floats_use_f64_and_round_half_up_with_ieee_specials() {
    for (bits, want) in [
        (0x0000, 0),
        (0x8000, 0),
        (0x0001, 0),
        (0x03ff, 0),
        (0x3800, 128),
        (0x37ff, 127),
        (0x3801, 128),
        (0x3c00, 255),
        (0x7c00, 255),
        (0xfc00, 0),
        (0x7e00, 0),
        (0x7c01, 0),
        (0xbc00, 0),
    ] {
        pixel(15, &u16::to_le_bytes(bits), &[want, 0, 0]);
    }
    for (bits, want) in [
        (0x0000_0000, 0),
        (0x8000_0000, 0),
        (0x0000_0001, 0),
        (0x007f_ffff, 0),
        (0x3f00_0000, 128),
        (0x3eff_ffff, 127),
        (0x3f00_0001, 128),
        (0x3f80_0000, 255),
        (0x7f80_0000, 255),
        (0xff80_0000, 0),
        (0x7fc0_0000, 0),
        (0x7f80_0001, 0),
        (0xbf80_0000, 0),
    ] {
        pixel(18, &u32::to_le_bytes(bits), &[want, 0, 0]);
    }
    // Just below a byte boundary: doing the multiply in f32 wrongly rounds up.
    pixel(18, &0x3b00_8080u32.to_le_bytes(), &[0, 0, 0]);
    pixel(16, &[0, 0x34, 0, 0x3a], &[64, 191, 0]);
    pixel(
        17,
        &[0, 0x38, 0, 0x34, 0, 0x30, 0, 0x3a],
        &[128, 64, 32, 191],
    );
    pixel(19, &[0, 0, 0x80, 0x3e, 0, 0, 0x40, 0x3f], &[64, 191, 0]);
    let data: Vec<_> = [0.0f32, 0.5, 1.0, 0.25]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    pixel(23, &data[..12], &[0, 128, 255]);
    pixel(20, &data, &[0, 128, 255, 64]);
}

#[test]
fn integers_reduce_high_bytes_and_offset_signed_channels() {
    for (format, bytes, want) in [
        (9, vec![0xff, 0], vec![0, 0, 0]),
        (9, vec![0, 0xff], vec![255, 0, 0]),
        (62, vec![7, 200], vec![7, 200, 0]),
        (72, vec![0x34, 0x12, 0xcd, 0xab], vec![0x12, 0xab, 0]),
        (73, vec![0xff, 0xff, 0, 0xff, 0xff, 0], vec![255, 255, 0]),
        (
            74,
            vec![0xff, 0xff, 0, 0x80, 0xff, 0x7f, 0, 0],
            vec![255, 128, 127, 0],
        ),
        (75, vec![0x80], vec![0, 0, 0]),
        (76, vec![0, 0x7f], vec![128, 255, 0]),
        (77, vec![0x80, 0, 0x7f], vec![0, 128, 255]),
        (78, vec![0xff, 1, 0x81, 64], vec![127, 129, 1, 192]),
        (79, vec![0xff, 0xff], vec![127, 0, 0]),
        (80, vec![0, 1, 0, 0xff], vec![129, 127, 0]),
        (81, vec![0, 0x80, 0, 0, 0xff, 0x7f], vec![0, 128, 255]),
        (
            82,
            vec![0, 0, 0, 0, 0, 0, 0xff, 0x7f],
            vec![128, 128, 128, 255],
        ),
    ] {
        pixel(format, &bytes, &want);
    }
}

#[test]
fn shared_exponent_and_bt601_have_the_stated_channel_order() {
    pixel(22, &0x7800_0100u32.to_le_bytes(), &[128, 0, 0]);
    pixel(
        22,
        &(16u32 << 27 | 256 << 18 | 128 << 9 | 1).to_le_bytes(),
        &[1, 128, 255],
    );
    let format = Format::from_texture_format(21).expect("YUY2");
    let image = decode(
        format,
        &[16, 128, 235, 128, 81, 90, 81, 240],
        2,
        2,
        &Meter::unlimited(),
    )
    .unwrap();
    assert_eq!(
        image.pixels(),
        &[255, 0, 0, 255, 0, 0, 0, 0, 0, 255, 255, 255]
    );
}
