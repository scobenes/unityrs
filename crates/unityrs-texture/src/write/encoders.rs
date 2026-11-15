// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py. See LICENSE-UNITYPY.
//! Checked, charged encoder buffers. Parameters follow
//! `export/Texture2DConverter.py:83-115`; padding precedes the raw RGBA pack
//! (`export/Texture2DConverter.py:220-223`).
use super::{WriteError, ffi};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

// Calibration: release build, pinned scalar C++ configuration in build.rs,
// one thread, default bc7enc parameters and astcenc quality 100. Encode a
// fixed 120x120 RGBA image (LCG seed 1, wrapping u32 s = 1664525*s +
// 1013904223, each byte s >> 24), without flip or mips. Decode its encoded
// BC7 with decode(Format::Bc7), which charges 4 work units/pixel. Time the
// public writer/decoder including allocation and packing with process CPU
// clock(), warm each once, then repeat for >=0.5 CPU seconds and average.
// Measured t_encode/t_decode (same pixel count); no absolute host timings:
//   DXT1 2.077393, DXT5 2.289034, BC4 2.065056, BC5 2.118889,
//   BC7 56.203801, ETC1/3DS 3.662098/3.642963,
//   ETC2 RGB 7.509432, ETC2 RGBA 16.193247.
//   ASTC footprint   RGB ratio    RGBA ratio    K (both aliases)
//     4x4           3091.908037   2857.116430    12368
//     5x5           2510.297667   2374.355766    10042
//     6x6           2692.492537   2374.101387    10770
//     8x8           2478.782963   2511.984104    10048
//    10x10          3049.492732   3000.796470    12198
//    12x12          3305.937940   3203.564616    13224
// K = ceil(4 * ratio), taking the larger ratio for shared encoder aliases.
// Thus (pixels*K / encode CPU-second) / (4*pixels / decode CPU-second)
// is 1.00000..1.13412 across all measured formats, including aliases.
// Packing/conversion keep their separate existing work charges. This is a
// conservative calibration for this scene, not a CPU-time bound on all data.
const DXT1_WORK: u64 = 9;
const DXT5_WORK: u64 = 10;
const BC4_WORK: u64 = 9;
const BC5_WORK: u64 = 9;
const BC7_WORK: u64 = 225;
const ETC1_WORK: u64 = 15;
const ETC2_RGB_WORK: u64 = 31;
const ETC2_RGBA_WORK: u64 = 65;
const ASTC_WORK: [u64; 6] = [12368, 10042, 10770, 10048, 12198, 13224];

fn work_per_pixel(format: i32) -> Result<u64, WriteError> {
    Ok(match format {
        10 => DXT1_WORK,
        12 => DXT5_WORK,
        25 => BC7_WORK,
        26 => BC4_WORK,
        27 => BC5_WORK,
        34 | 60 => ETC1_WORK,
        45 => ETC2_RGB_WORK,
        47 => ETC2_RGBA_WORK,
        48..=59 => {
            let index = usize::try_from((format - 48) % 6)
                .map_err(|_| WriteError::Invalid("ASTC work footprint"))?;
            *ASTC_WORK
                .get(index)
                .ok_or(WriteError::Invalid("ASTC work footprint"))?
        }
        _ => return Err(WriteError::EncoderUnavailable(format)),
    })
}

pub(super) const fn is_encoded(format: i32) -> bool {
    matches!(format, 10 | 12 | 25..=27 | 34 | 45 | 47..=60)
}

fn block(format: i32) -> Result<(u32, usize), WriteError> {
    match format {
        10 | 26 | 34 | 45 | 60 => Ok((4, 8)),
        12 | 25 | 27 | 47 => Ok((4, 16)),
        48..=59 => {
            let size = [4, 5, 6, 8, 10, 12]
                .get(usize::try_from((format - 48) % 6).unwrap_or(0))
                .copied()
                .ok_or(WriteError::Invalid("ASTC footprint"))?;
            Ok((size, 16))
        }
        _ => Err(WriteError::EncoderUnavailable(format)),
    }
}

pub(super) fn padded_size(format: i32, width: u32, height: u32) -> Result<(u32, u32), WriteError> {
    let (block, _) = block(format)?;
    let round = |side: u32| {
        side.div_ceil(block)
            .checked_mul(block)
            .ok_or(WriteError::Invalid("padded side"))
    };
    let size = (round(width)?, round(height)?);
    if u64::from(size.0) * u64::from(size.1) > crate::MAX_PIXELS {
        return Err(WriteError::Invalid("padded dimensions exceed bounds"));
    }
    Ok(size)
}

pub(super) fn encode(
    rgba: &[u8],
    width: u32,
    height: u32,
    format: i32,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, WriteError> {
    let (block, bytes) = block(format)?;
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > i32::MAX as u32
        || height > i32::MAX as u32
        || !width.is_multiple_of(block)
        || !height.is_multiple_of(block)
        || pixels > crate::MAX_PIXELS
        || pixels * 4 != rgba.len() as u64
    {
        return Err(WriteError::Invalid("encoder dimensions or RGBA length"));
    }
    let blocks = u64::from(width / block) * u64::from(height / block);
    let length = usize::try_from(blocks * bytes as u64)
        .map_err(|_| WriteError::Invalid("encoder output size"))?;
    // Refuse work before allocating output or entering any C++ encoder.
    meter.count_decode(pixels.saturating_mul(work_per_pixel(format)?))?;
    let (mut output, charge) = MeteredVec::<u8>::with_capacity(meter, length)?.into_parts();
    // Capacity was charged and reserved exactly above; resize cannot allocate.
    output.resize(length, 0);
    let mut output = MeteredVec::from_charged_parts(output, charge)?;
    ffi::encode(
        rgba,
        width,
        height,
        format,
        block,
        output.as_mut_slice(),
        meter,
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_encoder_charges_pixels_times_its_measured_weight() {
        // Literal expectations also catch accidental per-block charging or
        // alias drift; two blocks distinguish pixels from block counts.
        for (format, side, k) in [
            (10, 4, 9),
            (12, 4, 10),
            (25, 4, 225),
            (26, 4, 9),
            (27, 4, 9),
            (34, 4, 15),
            (60, 4, 15),
            (45, 4, 31),
            (47, 4, 65),
            (48, 4, 12368),
            (49, 5, 10042),
            (50, 6, 10770),
            (51, 8, 10048),
            (52, 10, 12198),
            (53, 12, 13224),
            (54, 4, 12368),
            (55, 5, 10042),
            (56, 6, 10770),
            (57, 8, 10048),
            (58, 10, 12198),
            (59, 12, 13224),
        ] {
            let width = side * 2;
            let height = side;
            let pixels = u64::from(width) * u64::from(height);
            let rgba = vec![73; usize::try_from(pixels * 4).unwrap()];
            let meter = Meter::unlimited();
            let output = encode(&rgba, width, height, format, &meter).unwrap();
            assert_eq!(meter.decoded(), pixels * k, "format {format}");
            drop(output);
            assert_eq!(meter.used(), 0, "format {format}");

            // A one-unit shortfall must refuse before output or C++ context
            // allocations. No memory at all is ever granted on this path.
            let meter = Meter::with_decode_work(
                32 << 20,
                pixels * k - 1,
                Arc::new(unityrs_core::budget::Unbounded),
                unityrs_core::budget::Reservation::Input,
            );
            assert!(
                matches!(
                    encode(&rgba, width, height, format, &meter),
                    Err(WriteError::Charge(unityrs_core::limits::ChargeError::Limit(error)))
                        if error.kind() == unityrs_core::error::LimitKind::DecodeWork
                ),
                "format {format}"
            );
            assert_eq!(meter.granted(), 0, "format {format}");
        }
    }
}
