//! Bounded raw and native encoded writes over sizes, modes, mips and platforms.
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_texture::write::{Options, Source, image_to_texture2d};

const MODES: [&str; 23] = [
    "1", "L", "LA", "La", "P", "PA", "RGB", "RGBA", "RGBX", "RGBa", "LAB", "CMYK", "YCbCr", "HSV",
    "I", "F", "I;16", "I;16L", "I;16B", "I;16N", "BGR;15", "BGR;16", "BGR;24",
];
fn byte(input: &[u8], at: usize) -> u8 {
    input.get(at).copied().unwrap_or(0)
}
fn word(input: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(std::array::from_fn(|i| byte(input, at + i)))
}
fn run(
    source: &Source<'_>,
    options: Options<'_>,
    limit: u64,
) -> Result<(usize, u64, i32, i32), String> {
    let meter = Meter::with_decode_work(limit, 8 << 20, Arc::new(Unbounded), Reservation::Input);
    let result = image_to_texture2d(source, options, &meter);
    let answer = result
        .as_ref()
        .map(|r| {
            assert!(meter.used() >= r.data.len() as u64);
            (
                r.data.len(),
                r.data
                    .iter()
                    .fold(0u64, |h, &b| h.wrapping_mul(257).wrapping_add(u64::from(b))),
                r.format,
                r.mipmap_count,
            )
        })
        .map_err(ToString::to_string);
    drop(result);
    assert_eq!(meter.used(), 0, "writer leaked charges");
    answer
}
/// Header: width/height u32, format/mips i32, mode, platform, flip, exponent,
/// memory policy, size policy, then repeating source pixels. No generated
/// source exceeds 65,536 bytes; malformed lengths and huge dimensions still
/// reach the public validator. Every accepted request runs twice.
pub fn check(input: &[u8]) {
    let mut width = word(input, 0);
    let mut height = word(input, 4);
    if byte(input, 21) & 1 != 0 {
        width %= 65;
        height %= 65;
    }
    let mode = MODES[usize::from(byte(input, 16)) % MODES.len()];
    let channels = match mode {
        "1" | "L" | "P" => 1,
        "LA" | "La" | "PA" | "I;16" | "I;16L" | "I;16B" | "I;16N" | "BGR;15" | "BGR;16" => 2,
        "RGB" | "LAB" | "HSV" | "YCbCr" | "BGR;24" => 3,
        _ => 4,
    };
    let row = if mode == "1" {
        u64::from(width).div_ceil(8)
    } else {
        u64::from(width) * channels
    };
    let len = row.checked_mul(u64::from(height)).unwrap_or(u64::MAX);
    let generated = if len <= 65536 && byte(input, 21) & 2 != 0 {
        (0..len as usize)
            .map(|i| byte(input, 22 + i % input.len().saturating_sub(22).max(1)))
            .collect::<Vec<_>>()
    } else {
        input
            .get(22..input.len().min(22 + 65_536))
            .unwrap_or(&[])
            .to_vec()
    };
    let palette = std::array::from_fn(|n| {
        std::array::from_fn(|c| {
            byte(
                input,
                22 + (n * 4 + c) % input.len().saturating_sub(22).max(1),
            )
        })
    });
    let source = Source::new(width, height, mode, &generated, Some(&palette));
    if let Ok(source) = source {
        let mut blob = [0u8; 12];
        blob[8..].copy_from_slice(&u32::from(byte(input, 19)).to_le_bytes());
        let options = Options {
            format: word(input, 8).cast_signed(),
            mipmap_count: word(input, 12).cast_signed(),
            platform: match byte(input, 17) % 3 {
                0 => 0,
                1 => 11,
                _ => 38,
            },
            platform_blob: Some(&blob),
            flip: byte(input, 18) & 1 == 0,
        };
        let limit = match byte(input, 20) % 4 {
            0 => 0,
            1 => 128,
            2 => 4096,
            _ => 8 << 20,
        };
        assert_eq!(run(&source, options, limit), run(&source, options, limit));
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn all_formats_modes_and_platforms_with_small_and_normal_budgets() {
        for fmt in 0..=83i32 {
            for mode in 0..23u8 {
                for platform in 0..3u8 {
                    for memory in 0..4u8 {
                        let mut input = Vec::new();
                        // Native encoders have dedicated small scenes below;
                        // keep the broad mode/platform matrix inexpensive.
                        if matches!(fmt, 10 | 12 | 25..=29 | 34 | 45..=60 | 64..=71) {
                            continue;
                        }
                        input.extend_from_slice(&17u32.to_le_bytes());
                        input.extend_from_slice(&19u32.to_le_bytes());
                        input.extend_from_slice(&fmt.to_le_bytes());
                        let mips: i32 = if platform == 2 { 1 } else { 4 };
                        input.extend_from_slice(&mips.to_le_bytes());
                        input
                            .extend_from_slice(&[mode, platform, 0, 1, memory, 2, 73, 0, 255, 128]);
                        super::check(&input);
                    }
                }
            }
        }
    }
    #[test]
    fn native_encoders_release_charges_and_repeat_their_bytes() {
        for fmt in [
            10i32, 12, 25, 26, 27, 34, 45, 47, 48, 49, 50, 51, 52, 53, 54, 59, 60, 66, 71,
        ] {
            for memory in [0, 1, 3] {
                let mut input = Vec::new();
                input.extend_from_slice(&4u32.to_le_bytes());
                input.extend_from_slice(&4u32.to_le_bytes());
                input.extend_from_slice(&fmt.to_le_bytes());
                input.extend_from_slice(&1i32.to_le_bytes());
                input.extend_from_slice(&[7, 0, 0, 0, memory, 2, 73, 0, 255, 128]);
                super::check(&input);
            }
        }
    }
    #[test]
    fn malformed_sizes_and_storage_never_panic() {
        for n in 0..80 {
            super::check(&vec![255; n]);
        }
        super::check(&[]);
    }
}
