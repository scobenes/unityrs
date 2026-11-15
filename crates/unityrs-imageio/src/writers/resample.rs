// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/Resample.c,
// src/libImaging/Convert.c, src/libImaging/Geometry.c's ImagingScaleAffine,
// and src/PIL/Image.py's resize and thumbnail. See LICENSE-PILLOW.
//! Icon resizing with Pillow's fixed point separable filters.
// Convert.c: Copyright (c) 1997-2005 by Secret Labs AB.
// Copyright (c) 1995-1997 by Fredrik Lundh.
// Geometry.c: Copyright (c) Fredrik Lundh 1999.
// Copyright (c) Secret Labs AB 1999.

use super::{Exception, Refusal, SaveError};
use crate::{Buffer, Charger, EncodeError, Image, ImageError, Mode};

#[derive(Clone, Copy)]
pub(super) enum Filter {
    Bicubic,
    Lanczos,
}

pub(super) struct Resized<C> {
    pub bytes: Vec<u8>,
    _charge: C,
}

pub(super) fn resize<C: Charger>(
    image: &Image<'_>,
    width: u32,
    height: u32,
    filter: Filter,
    charger: &mut C,
) -> Result<Resized<C::Charge>, SaveError<C::Refusal>> {
    if width == 0 || height == 0 {
        return Err(Refusal::new(Exception::ValueError, "height and width must be > 0").into());
    }
    let channels = image.mode().channels();
    let length = area(image, width, height, channels)?;
    let output_charge = charge(charger, &[length])?;
    let mut bytes = allocated(length, 0_u8)?;
    if width == image.width() && height == image.height() {
        bytes.copy_from_slice(image.pixels());
    } else if image.mode() == Mode::P {
        if image.width() != 0 && image.height() != 0 {
            nearest(image, width, height, &mut bytes);
        }
    } else {
        // Resample.c precomputes horizontal, then vertical coefficients before
        // either pass, even for an empty source. P uses nearest with no tables.
        let horizontal = kernels(image, image.width(), width, filter, charger)?;
        let vertical = kernels(image, image.height(), height, filter, charger)?;
        if image.width() == 0 || image.height() == 0 {
            return Ok(Resized {
                bytes,
                _charge: output_charge,
            });
        }
        let temporary_length = area(image, width, image.height(), channels)?;
        let _temporary_charge = charge(charger, &[temporary_length])?;
        let mut temporary = allocated(temporary_length, 0_u8)?;
        for (row_number, row) in temporary
            .chunks_exact_mut(width as usize * channels)
            .enumerate()
        {
            for (x, (coeffs, &(start, count))) in horizontal
                .coeffs
                .chunks_exact(horizontal.stride)
                .zip(&horizontal.bounds)
                .enumerate()
            {
                for channel in 0..channels {
                    let mut sum = 1_i64 << 21;
                    for (offset, &weight) in coeffs.iter().take(count).enumerate() {
                        let index = (row_number * image.width() as usize + start + offset)
                            * channels
                            + channel;
                        sum += i64::from(sample(image, index, channel)) * i64::from(weight);
                    }
                    if let Some(slot) = row.get_mut(x * channels + channel) {
                        *slot = clip(sum);
                    }
                }
            }
        }
        let row_length = width as usize * channels;
        for ((row, coeffs), &(start, count)) in bytes
            .chunks_exact_mut(row_length)
            .zip(vertical.coeffs.chunks_exact(vertical.stride))
            .zip(&vertical.bounds)
        {
            for (column, slot) in row.iter_mut().enumerate() {
                let mut sum = 1_i64 << 21;
                for (offset, &weight) in coeffs.iter().take(count).enumerate() {
                    let index = (start + offset) * row_length + column;
                    sum += i64::from(temporary.get(index).copied().unwrap_or_default())
                        * i64::from(weight);
                }
                *slot = clip(sum);
            }
        }
        if matches!(image.mode(), Mode::La | Mode::Rgba) {
            for pixel in bytes.chunks_exact_mut(channels) {
                let alpha = pixel.last().copied().unwrap_or_default();
                if alpha != 0 && alpha != 255 {
                    for color in pixel.iter_mut().take(channels - 1) {
                        *color = (u32::from(*color) * 255 / u32::from(alpha)).min(255) as u8;
                    }
                }
            }
        }
    }
    Ok(Resized {
        bytes,
        _charge: output_charge,
    })
}

// Pillow truncates its nonnegative source coordinates to pixel indices.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn nearest(image: &Image<'_>, width: u32, height: u32, output: &mut [u8]) {
    // Geometry.c's ImagingScaleAffine accumulates coordinates in doubles.
    let scale_x = f64::from(image.width() as f32) / f64::from(width);
    let scale_y = f64::from(image.height() as f32) / f64::from(height);
    let mut y = scale_y * 0.5;
    for row in output.chunks_exact_mut(width as usize) {
        let mut x = scale_x * 0.5;
        for slot in row {
            let index = (y as usize) * image.width() as usize + x as usize;
            *slot = image.pixels().get(index).copied().unwrap_or_default();
            x += scale_x;
        }
        y += scale_y;
    }
}

fn sample(image: &Image<'_>, index: usize, channel: usize) -> u8 {
    let value = image.pixels().get(index).copied().unwrap_or_default();
    let channels = image.mode().channels();
    if matches!(image.mode(), Mode::La | Mode::Rgba) && channel + 1 != channels {
        let alpha = image
            .pixels()
            .get(index - channel + channels - 1)
            .copied()
            .unwrap_or_default();
        let temporary = u32::from(value) * u32::from(alpha) + 128;
        u8::try_from((temporary + (temporary >> 8)) >> 8).unwrap_or_default()
    } else {
        value
    }
}

fn clip(sum: i64) -> u8 {
    u8::try_from((sum >> 22).clamp(0, 255)).unwrap_or_default()
}

struct Kernels<C> {
    coeffs: Vec<i32>,
    bounds: Vec<(usize, usize)>,
    stride: usize,
    _charge: C,
}

// These conversions implement C's float box, integer bounds, and normalized
// 22-bit coefficients. All indices are bounded by the input's u32 extent.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn kernels<C: Charger>(
    image: &Image<'_>,
    input: u32,
    output: u32,
    filter: Filter,
    charger: &mut C,
) -> Result<Kernels<C::Charge>, SaveError<C::Refusal>> {
    // The Python box enters C as floats, including the source's extent.
    let scale = f64::from(input as f32) / f64::from(output);
    let filter_scale = scale.max(1.0);
    let support = match filter {
        Filter::Bicubic => 2.0,
        Filter::Lanczos => 3.0,
    } * filter_scale;
    let stride = if input == output {
        1
    } else {
        (support.ceil() as usize)
            .checked_mul(2)
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| Refusal::new(Exception::MemoryError, ""))?
    };
    // Pillow allocates doubles before normalizing to fixed point, and refuses
    // when outSize > INT_MAX / (ksize * sizeof(double)). The float box above
    // matters at the boundary. Our unchanged axis uses an identity kernel.
    if input != output
        && stride
            .checked_mul(8)
            .is_none_or(|bytes| output as usize > i32::MAX as usize / bytes)
    {
        return Err(Refusal::new(Exception::MemoryError, "").into());
    }
    let count = (output as usize)
        .checked_mul(stride)
        .ok_or_else(|| too_large(image))?;
    let coefficient_bytes = count
        .checked_mul(size_of::<i32>())
        .ok_or_else(|| too_large(image))?;
    let bounds_bytes = (output as usize)
        .checked_mul(size_of::<(usize, usize)>())
        .ok_or_else(|| too_large(image))?;
    let coefficient_charge = charge(charger, &[coefficient_bytes, bounds_bytes])?;
    let mut coeffs = allocated(count, 0_i32)?;
    let mut bounds = allocated(output as usize, (0_usize, 0_usize))?;
    for (x, (row, bound)) in coeffs.chunks_exact_mut(stride).zip(&mut bounds).enumerate() {
        if input == output {
            *bound = (x, 1);
            if let Some(coefficient) = row.first_mut() {
                *coefficient = 1 << 22;
            }
            continue;
        }
        let center = (x as f64 + 0.5) * scale;
        let start = ((center - support + 0.5) as i64).max(0) as usize;
        let stop = ((center + support + 0.5) as usize).min(input as usize);
        let count = stop.saturating_sub(start);
        *bound = (start, count);
        let inverse_scale = 1.0 / filter_scale;
        let evaluate = |offset: usize| {
            weight(
                filter,
                (offset as f64 + start as f64 - center + 0.5) * inverse_scale,
            )
        };
        let mut sum = 0.0;
        for offset in 0..count {
            sum += evaluate(offset);
        }
        for (offset, coefficient) in row.iter_mut().take(count).enumerate() {
            let value = evaluate(offset);
            let normalized = if sum == 0.0 { value } else { value / sum } * f64::from(1 << 22);
            *coefficient = if normalized < 0.0 {
                normalized - 0.5
            } else {
                normalized + 0.5
            } as i32;
        }
    }
    Ok(Kernels {
        coeffs,
        bounds,
        stride,
        _charge: coefficient_charge,
    })
}

fn weight(filter: Filter, value: f64) -> f64 {
    match filter {
        Filter::Bicubic => {
            let x = value.abs();
            if x < 1.0 {
                ((1.5 * x - 2.5) * x) * x + 1.0
            } else if x < 2.0 {
                (((x - 5.0) * x + 8.0) * x - 4.0) * -0.5
            } else {
                0.0
            }
        }
        Filter::Lanczos => {
            let sinc = |x: f64| {
                if x == 0.0 {
                    1.0
                } else {
                    let angle = x * std::f64::consts::PI;
                    angle.sin() / angle
                }
            };
            if (-3.0..3.0).contains(&value) {
                sinc(value) * sinc(value / 3.0)
            } else {
                0.0
            }
        }
    }
}

fn area<R>(
    image: &Image<'_>,
    width: u32,
    height: u32,
    channels: usize,
) -> Result<usize, EncodeError<R>> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(channels))
        .ok_or_else(|| too_large(image))
}

fn too_large<R>(image: &Image<'_>) -> EncodeError<R> {
    ImageError::TooLarge {
        width: image.width(),
        height: image.height(),
    }
    .into()
}

fn charge<C: Charger>(
    charger: &mut C,
    requests: &[usize],
) -> Result<C::Charge, EncodeError<C::Refusal>> {
    charger
        .charge(Buffer::Rows, requests)
        .map_err(|refusal| EncodeError::Refused {
            buffer: Buffer::Rows,
            refusal,
        })
}

fn allocated<T: Clone, R>(length: usize, value: T) -> Result<Vec<T>, EncodeError<R>> {
    let request = length.saturating_mul(size_of::<T>());
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| EncodeError::Alloc {
            buffer: Buffer::Rows,
            request,
        })?;
    bytes.resize(length, value);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Filter, kernels, resize};
    use crate::{Buffer, Charger, Image, Mode, Unmetered};

    struct SmallBudget;

    impl Charger for SmallBudget {
        type Charge = ();
        type Refusal = &'static str;

        fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<(), Self::Refusal> {
            if requests.iter().sum::<usize>() > 1_048_576 {
                Err("test budget")
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn empty_destination_is_refused_before_resampling() {
        let image = Image::new(1, 1, Mode::L, &[17]).unwrap();
        for mode in [Mode::L, Mode::P] {
            let image = Image::new(1, 1, mode, image.pixels()).unwrap();
            for (width, height) in [(0, 1), (1, 0), (0, 0)] {
                for filter in [Filter::Bicubic, Filter::Lanczos] {
                    let error = resize(&image, width, height, filter, &mut SmallBudget)
                        .err()
                        .expect("empty destination must be refused");
                    assert_eq!(
                        error.to_string(),
                        "ValueError: height and width must be > 0"
                    );
                }
            }
        }
    }

    #[test]
    fn coefficient_overflow_precedes_allocation_at_pillows_float_boundaries() {
        // The budget stops accepted tables before allocating about 1 GiB.
        let image = Image::new(0, 0, Mode::L, &[]).unwrap();
        for (filter, output, accepted, refused) in [
            (Filter::Bicubic, 1024, 67_108_353, 67_108_360),
            (Filter::Lanczos, 16, 44_739_237, 44_739_238),
        ] {
            let error = kernels(&image, refused, output, filter, &mut SmallBudget)
                .err()
                .expect("Pillow must refuse coefficient overflow");
            assert_eq!(error.to_string(), "MemoryError: ");
            let error = kernels(&image, accepted, output, filter, &mut SmallBudget)
                .err()
                .expect("accepted table must reach the allocation budget");
            assert!(format!("{error:?}").contains("test budget"), "{error}");
        }
    }

    #[test]
    fn both_coefficient_tables_are_checked_even_for_an_empty_source() {
        for (width, height) in [(68_000_000, 0), (0, 68_000_000)] {
            let image = Image::new(width, height, Mode::Rgba, &[]).unwrap();
            let error = resize(&image, 128, 128, Filter::Bicubic, &mut SmallBudget)
                .err()
                .expect("empty source must still check both tables");
            assert_eq!(error.to_string(), "MemoryError: ");
            let image = Image::new(width, height, Mode::P, &[]).unwrap();
            assert_eq!(
                resize(&image, 128, 128, Filter::Bicubic, &mut SmallBudget)
                    .unwrap()
                    .bytes,
                vec![0; 128 * 128]
            );
        }
    }

    #[test]
    fn large_resizes_replay_the_recorded_pillow_boundaries() {
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/writers.json")).unwrap();
        let mut count = 0;
        for case in fixture["cases"].as_array().unwrap() {
            if case["policy"] != "resize" {
                continue;
            }
            count += 1;
            let width = u32::try_from(case["width"].as_u64().unwrap()).unwrap();
            let height = u32::try_from(case["height"].as_u64().unwrap()).unwrap();
            let pixels = vec![73; width as usize * height as usize];
            let image = Image::new(width, height, Mode::L, &pixels).unwrap();
            let resize_case = &case["resize"];
            let width = u32::try_from(resize_case[0].as_u64().unwrap()).unwrap();
            let height = u32::try_from(resize_case[1].as_u64().unwrap()).unwrap();
            let filter = match resize_case[2].as_str().unwrap() {
                "BICUBIC" => Filter::Bicubic,
                "LANCZOS" => Filter::Lanczos,
                other => panic!("unexpected filter {other}"),
            };
            match resize(&image, width, height, filter, &mut Unmetered) {
                Ok(resized) => {
                    assert!(case.get("exception").is_none(), "{case}");
                    let image = Image::new(width, height, Mode::L, &resized.bytes).unwrap();
                    let png = crate::encode(&image, &mut Unmetered).unwrap();
                    assert_eq!(png.bytes().len() as u64, case["len"].as_u64().unwrap());
                    let expected: Vec<u8> = case["sha256"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| {
                            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                        })
                        .collect();
                    assert_eq!(Sha256::digest(png.bytes()).as_slice(), expected);
                }
                Err(super::SaveError::Pillow(error)) => {
                    assert_eq!(error.class.name(), case["exception"].as_str().unwrap());
                    assert_eq!(error.message, case["message"].as_str().unwrap());
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert_eq!(count, 4);
    }

    #[test]
    fn pinned_pillow_filters_include_premultiplied_alpha() {
        // Oracle: Pillow 9.5.0 Image.resize((2,3), BICUBIC/LANCZOS).
        let cases: &[(Mode, &[u8], &[u8])] = &[
            (
                Mode::L,
                &[13, 125, 97, 109, 181, 93],
                &[2, 130, 95, 108, 187, 86],
            ),
            (
                Mode::La,
                &[96, 137, 106, 175, 115, 117, 88, 155, 141, 97, 66, 135],
                &[98, 142, 110, 182, 119, 120, 90, 160, 148, 98, 64, 138],
            ),
            (
                Mode::Rgb,
                &[
                    77, 71, 144, 240, 36, 120, 140, 35, 113, 204, 99, 89, 203, 0, 82, 168, 162, 58,
                ],
                &[
                    70, 76, 148, 252, 28, 122, 141, 34, 114, 207, 99, 88, 211, 0, 80, 162, 169, 54,
                ],
            ),
            (
                Mode::Rgba,
                &[
                    0, 79, 159, 163, 105, 218, 36, 63, 115, 48, 129, 179, 174, 125, 112, 79, 221,
                    22, 103, 195, 217, 64, 163, 95,
                ],
                &[
                    0, 79, 165, 160, 113, 255, 4, 54, 116, 47, 130, 182, 184, 130, 117, 76, 231,
                    21, 102, 204, 223, 62, 176, 98,
                ],
            ),
        ];
        for &(mode, cubic, lanczos) in cases {
            let pixels: Vec<u8> = (0..6 * mode.channels())
                .map(|i| u8::try_from((i * 73 + i / 3 * 19) % 256).unwrap())
                .collect();
            let image = Image::new(3, 2, mode, &pixels).unwrap();
            for (filter, expected) in [(Filter::Bicubic, cubic), (Filter::Lanczos, lanczos)] {
                assert_eq!(
                    resize(&image, 2, 3, filter, &mut Unmetered).unwrap().bytes,
                    expected,
                    "{mode}"
                );
            }
        }
    }
}
