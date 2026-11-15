// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py. See LICENSE-UNITYPY.
//! Checked, charged encoder buffers. Parameters follow
//! `export/Texture2DConverter.py:83-115`; padding precedes the raw RGBA pack
//! (`export/Texture2DConverter.py:220-223`).
use super::{WriteError, ffi};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

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
    // Fixed quality bounds the work per block. Charge before entering C++.
    meter.count_decode(blocks.saturating_mul(if format >= 48 { 4096 } else { 256 }))?;
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
