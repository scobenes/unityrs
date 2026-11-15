//! Bounded default-image-writer checks: malformed dimensions and pixel counts,
//! deterministic output and refusals, palette handling, and released charges.
use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{Charge, ChargeError, Meter, heap_cost};
use unityrs_imageio::writers::{self, Format, Refusal, SaveError};
use unityrs_imageio::{Buffer, Charger, Image, Mode};

const MAX_PIXELS: u64 = 65_536;
const MEMORY: u64 = 4 << 20;
const MODES: [Mode; 5] = [Mode::L, Mode::La, Mode::Rgb, Mode::Rgba, Mode::P];
const FORMATS: [Format; 19] = [
    Format::Bmp,
    Format::Dib,
    Format::Tga,
    Format::Ppm,
    Format::Pcx,
    Format::Sgi,
    Format::Im,
    Format::Tiff,
    Format::Dds,
    Format::Ico,
    Format::Icns,
    Format::Eps,
    Format::Png,
    Format::Gif,
    Format::Jpeg,
    Format::Mpo,
    Format::Pdf,
    Format::Jpeg2000,
    Format::Webp,
];
const SIDES: [u32; 14] = [
    0,
    1,
    2,
    3,
    15,
    16,
    17,
    255,
    256,
    65534,
    65535,
    65536,
    i32::MAX as u32,
    u32::MAX,
];

struct Budget(Arc<Meter>);
impl Charger for Budget {
    type Charge = Charge;
    type Refusal = ChargeError;
    fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<Charge, ChargeError> {
        self.0.charge(
            requests
                .iter()
                .map(|&n| heap_cost(n as u64))
                .fold(0, u64::saturating_add),
        )
    }
}
fn meter(limit: u64) -> Arc<Meter> {
    Meter::new(limit, Arc::new(Unbounded), Reservation::Input)
}
fn byte(input: &[u8], at: usize) -> u8 {
    input.get(at).copied().unwrap_or(0)
}
fn word(input: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(std::array::from_fn(|n| byte(input, at + n)))
}
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Bytes(usize, u64),
    Pillow(Refusal),
    Unsupported(Format),
    Encode,
}
fn run(image: &Image<'_>, format: Format, filename: &str, limit: u64) -> Answer {
    let meter = meter(limit);
    let result = writers::save(image, format, filename, &mut Budget(meter.clone()));
    let answer = match result {
        Ok(saved) => {
            assert!(
                meter.used() >= saved.bytes().len() as u64,
                "unaccounted output"
            );
            Answer::Bytes(saved.bytes().len(), hash(saved.bytes()))
        }
        Err(SaveError::Pillow(error)) => Answer::Pillow(error),
        Err(SaveError::Unsupported { format }) => Answer::Unsupported(format),
        Err(SaveError::Encode(_) | SaveError::Webp(_)) => Answer::Encode,
    };
    assert_eq!(meter.used(), 0, "writer leaked a charge");
    answer
}

/// A panic, differing replay, accepted malformed pixels, or leaked charge is a
/// finding. Work is bounded by 65,536 source pixels; ICNS only sees a charger
/// refusing every allocation because its mandatory 1024-square resize is costly.
/// Input bytes: width/height u32 LE, mode, format, dimension policy, budget,
/// filename selector, then repeating pixel and palette bytes.
pub fn check(input: &[u8]) {
    let mode = MODES
        .get(usize::from(byte(input, 8)) % MODES.len())
        .copied()
        .unwrap_or(Mode::L);
    let format = FORMATS
        .get(usize::from(byte(input, 9)) % FORMATS.len())
        .copied()
        .unwrap_or(Format::Bmp);
    assert_eq!(Format::from_name(format.name()), Some(format));
    let raw_width = word(input, 0);
    let raw_height = word(input, 4);
    let source = input.get(13..).unwrap_or_default();
    let source = source
        .get(..source.len().min(MAX_PIXELS as usize * 4))
        .unwrap_or_default();
    let expected = u64::from(raw_width)
        .checked_mul(u64::from(raw_height))
        .and_then(|n| n.checked_mul(mode.channels() as u64));
    let malformed = Image::new(raw_width, raw_height, mode, source);
    assert_eq!(malformed.is_ok(), expected == Some(source.len() as u64));
    let mut palette = [0_u8; 768];
    for (n, value) in palette.iter_mut().enumerate() {
        *value = source
            .get(n % source.len().max(1))
            .copied()
            .unwrap_or(n as u8);
    }
    let paletted = Image::with_palette(raw_width, raw_height, source, &palette);
    let expected_p = u64::from(raw_width).checked_mul(u64::from(raw_height));
    assert_eq!(paletted.is_ok(), expected_p == Some(source.len() as u64));

    let (mut width, mut height) = match byte(input, 10) % 3 {
        0 => (raw_width, raw_height),
        1 => (raw_width % 65, raw_height % 65),
        _ => (
            SIDES
                .get(raw_width as usize % SIDES.len())
                .copied()
                .unwrap_or(0),
            SIDES
                .get(raw_height as usize % SIDES.len())
                .copied()
                .unwrap_or(0),
        ),
    };
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        width %= 65;
        height %= 65;
    }
    let len = (u64::from(width) * u64::from(height) * mode.channels() as u64) as usize;
    let input_meter = meter(MEMORY);
    let Ok(input_charge) = input_meter.charge(heap_cost(len as u64)) else {
        return;
    };
    let mut pixels = Vec::new();
    if pixels.try_reserve_exact(len).is_err() {
        return;
    }
    for n in 0..len {
        pixels.push(
            source
                .get(n % source.len().max(1))
                .copied()
                .unwrap_or(n as u8),
        );
    }
    let image = if mode == Mode::P && byte(input, 10) & 4 != 0 {
        Image::with_palette(width, height, &pixels, &palette)
    } else {
        Image::new(width, height, mode, &pixels)
    };
    if let Ok(image) = image {
        let filename = match byte(input, 12) % 4 {
            0 => "fuzz",
            1 => "folder/a.b.c",
            2 => "folder/é-image",
            _ => "folder/😀",
        };
        let large = if format == Format::Icns { 0 } else { MEMORY };
        let first = run(&image, format, filename, large);
        assert_eq!(
            first,
            run(&image, format, filename, large),
            "writer replay differs"
        );
        let small = [0, 1, 64, 4096]
            .get(usize::from(byte(input, 11)) % 4)
            .copied()
            .unwrap_or(0);
        let _ = run(
            &image,
            format,
            filename,
            if format == Format::Icns { 0 } else { small },
        );
    } else {
        panic!("bounded exact pixels rejected");
    }
    drop(pixels);
    drop(input_charge);
    assert_eq!(input_meter.used(), 0, "input pixels leaked a charge");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_ppm_and_pcx_replay() {
        let pixels = [1, 2, 3, 4, 5, 6];
        let image = Image::new(2, 1, Mode::Rgb, &pixels).unwrap();
        let expected = b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06";
        assert_eq!(
            run(&image, Format::Ppm, "fuzz", MEMORY),
            Answer::Bytes(expected.len(), hash(expected))
        );
        assert_eq!(run(&image, Format::Ppm, "fuzz", 0), Answer::Encode);
        // Pinned Pillow 9.5.0 emits only R and G planes at RGB width one.
        let image = Image::new(1, 1, Mode::Rgb, &pixels[..3]).unwrap();
        let saved =
            writers::save(&image, Format::Pcx, "fuzz", &mut unityrs_imageio::Unmetered).unwrap();
        assert_eq!(saved.bytes().get(128..), Some([1, 0, 2, 0].as_slice()));
    }

    #[test]
    fn bounded_cases_cover_modes_formats_dimensions_and_refusals() {
        for format in 0..FORMATS.len() {
            for mode in 0..MODES.len() {
                for side in [0_u32, 1, 3, 16, 65535, 65536, u32::MAX] {
                    let mut input = [0_u8; 29];
                    input[..4].copy_from_slice(&side.to_le_bytes());
                    input[4..8].copy_from_slice(&1_u32.to_le_bytes());
                    input[8] = mode as u8;
                    input[9] = format as u8;
                    input[11] = 2;
                    input[12] = 2;
                    input[13..].fill(0xc0);
                    check(&input);
                }
            }
        }
        for end in 0..13 {
            check(&[0; 13][..end]);
        }
    }
}
