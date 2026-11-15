// Ported from UnityPy 1.25.2 (MIT): helpers/TextureSwizzler.py and export/Texture2DConverter.py's platform preprocessing. See THIRD_PARTY_LICENSES.
//! Switch GOB layout and Xbox byte pairs, before crunch/decode/crop.
//! The table and GOB order are `helpers/TextureSwizzler.py:6-10`,
//! `:73-103`; padding is `:106-114`, deswizzle `:17-41`.
//! Converter ordering and RGB promotion: `export/Texture2DConverter.py:254-320`.
use std::sync::Arc;

use unityrs_core::limits::{Meter, MeteredVec, heap_cost};

use crate::{DecodeError, Format, Image, MAX_CROP_PIXELS, MAX_PIXELS, Plan, TextureFields};

/// Platform requests with no reference, or sizes that cannot be processed safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// The Switch table has no 16-byte texel block for this format (Tier U).
    #[error("unsupported Switch texel block for TextureFormat {0}")]
    SwitchFormat(i32),
    /// No decoder for this `TextureFormat` is linked.
    #[error("unsupported TextureFormat {0}")]
    TextureFormat(i32),
    /// The blob's exponent cannot be shifted or its padded size represented.
    #[error("Switch GOB exponent {0} exceeds the bounded layout")]
    GobsExponent(u32),
    /// An image's dimensions or buffer length cannot be represented.
    #[error("image buffer size overflow")]
    SizeOverflow,
    /// The helper only grows images; shrinking has no padding interpretation.
    #[error("padding dimensions must include the original image")]
    PaddingSize,
}
const _: () = crate::send_sync::<PlatformError>();

/// The complete 29-entry 16-byte texel table, independent of linked decoders.
#[must_use]
pub const fn switch_block(format: i32) -> Option<(u32, u32)> {
    match format {
        1 | 63 => Some((16, 1)),
        2 | 7 | 9 | 13 | 62 => Some((8, 1)),
        4 | 5 | 14 => Some((4, 1)),
        6 => Some((1, 1)),
        10 | 26 => Some((8, 4)),
        12 | 24 | 25 | 27 | 48 | 54 => Some((4, 4)),
        49 | 55 => Some((5, 5)),
        50 | 56 => Some((6, 6)),
        51 | 57 => Some((8, 8)),
        52 | 58 => Some((10, 10)),
        53 | 59 => Some((12, 12)),
        _ => None,
    }
}

/// Checked dimensions and strides for Switch's 4 by 8 GOBs of 16-byte texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchLayout {
    width: u32,
    height: u32,
    block: (u32, u32),
    gobs: u32,
    bytes: usize,
}
const _: () = crate::send_sync::<SwitchLayout>();
impl SwitchLayout {
    /// Dimensions padded to whole GOB blocks.
    #[must_use]
    pub const fn padded_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    /// Pixels held by one 16-byte texel.
    #[must_use]
    pub const fn block(&self) -> (u32, u32) {
        self.block
    }
    /// Number of bytes in the padded top level.
    #[must_use]
    pub const fn input_bytes(&self) -> usize {
        self.bytes
    }
}

/// Pad to whole GOB blocks using the blob's little-endian exponent.
pub fn switch_layout(
    format: i32,
    width: i32,
    height: i32,
    exponent: u32,
) -> Result<SwitchLayout, DecodeError> {
    let block = switch_block(format).ok_or(PlatformError::SwitchFormat(format))?;
    if width < 0 || height < 0 {
        return Err(DecodeError::NegativeSize { width, height });
    }
    let gobs = 1u32
        .checked_shl(exponent)
        .ok_or(PlatformError::GobsExponent(exponent))?;
    let x = u64::from(block.0) * 4;
    let y = u64::from(block.1) * 8 * u64::from(gobs);
    let w = u64::from(width.unsigned_abs()).div_ceil(x) * x;
    let h = u64::from(height.unsigned_abs()).div_ceil(y) * y;
    let pixels = w.checked_mul(h).ok_or(PlatformError::SizeOverflow)?;
    if pixels > MAX_PIXELS {
        return Err(DecodeError::TooLarge {
            pixels,
            max: MAX_PIXELS,
        });
    }
    let bytes = w / u64::from(block.0) * (h / u64::from(block.1)) * 16;
    Ok(SwitchLayout {
        width: u32::try_from(w).map_err(|_| PlatformError::SizeOverflow)?,
        height: u32::try_from(h).map_err(|_| PlatformError::SizeOverflow)?,
        block,
        gobs,
        bytes: usize::try_from(bytes).map_err(|_| PlatformError::SizeOverflow)?,
    })
}

/// Deswizzle mip zero; preserve UnityPy's output length and zeroed tail.
/// Short padded data is refused before allocating; Python's resizing slice
/// assignments on truncated data have no bounded texture interpretation.
pub fn deswizzle(
    data: &[u8],
    layout: &SwitchLayout,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, DecodeError> {
    if data.len() < layout.bytes {
        return Err(DecodeError::NotEnoughData {
            needed: layout.bytes as u64,
            got: data.len() as u64,
        });
    }
    meter.count_decode((data.len() as u64).saturating_add(layout.bytes as u64))?;
    let mut out = MeteredVec::with_capacity(meter, data.len())?;
    for _ in 0..data.len() {
        out.push(0)?;
    }
    let bx = u64::from(layout.width / layout.block.0);
    let by = u64::from(layout.height / layout.block.1);
    let gobs = u64::from(layout.gobs);
    let mut src = data.as_chunks::<16>().0.iter();
    for i in 0..by / 8 / gobs {
        for j in 0..bx / 4 {
            for k in 0..gobs {
                for v in 0..32u64 {
                    let x = ((v >> 3) & 2) | ((v >> 1) & 1);
                    let y = ((v >> 1) & 6) | (v & 1);
                    let offset = (((i * gobs + k) * 8 + y) * bx + j * 4 + x) * 16;
                    let at = usize::try_from(offset).map_err(|_| PlatformError::SizeOverflow)?;
                    let dst = out
                        .as_mut_slice()
                        .get_mut(at..at + 16)
                        .ok_or(PlatformError::SizeOverflow)?;
                    dst.copy_from_slice(src.next().ok_or(PlatformError::SizeOverflow)?);
                }
            }
        }
    }
    Ok(out)
}

/// Swap every pair, keeping an odd last byte (`export/Texture2DConverter.py:323-328`).
pub fn swap_bytes_for_xbox(data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u8>, DecodeError> {
    meter.count_decode(data.len() as u64)?;
    let mut out = MeteredVec::with_capacity(meter, data.len())?;
    for pair in data.chunks(2) {
        for &byte in pair.iter().rev() {
            out.push(byte)?;
        }
    }
    Ok(out)
}

fn effective(format: i32) -> i32 {
    match format {
        3 => 4,
        8 => 14,
        other => other,
    }
}

fn prepared(
    t: &TextureFields<'_>,
    len: usize,
    selected: Option<Format>,
) -> Result<(Format, Plan, Option<SwitchLayout>), DecodeError> {
    // Empty sides precede even format/platform checks in the converter.
    if t.width == 0 || t.height == 0 {
        return Ok((
            Format::Rgba32,
            crate::plan(Format::Rgba32, len, 0, 0)?,
            None,
        ));
    }
    if t.texture_format == 6 || (t.texture_format == 21 && t.width % 2 != 0) {
        return Err(DecodeError::Unsupported {
            format: if t.texture_format == 6 {
                Format::ArgbFloat
            } else {
                Format::Yuy2
            },
            width: t.width,
        });
    }
    if len == 0 {
        return Err(DecodeError::NoImageData);
    }
    let switch = crate::is_switch_swizzled(t.platform, t.platform_blob);
    let value = if switch {
        effective(t.texture_format)
    } else {
        t.texture_format
    };
    let layout = if switch {
        let blob = t
            .platform_blob
            .and_then(|b| b.get(8..12))
            .ok_or(PlatformError::SizeOverflow)?;
        let exponent =
            u32::from_le_bytes(blob.try_into().map_err(|_| PlatformError::SizeOverflow)?);
        Some(switch_layout(value, t.width, t.height, exponent)?)
    } else {
        None
    };
    let format = selected
        .filter(|f| f.texture_format() == value)
        .or_else(|| Format::from_texture_format(value))
        .ok_or(PlatformError::TextureFormat(value))?;
    let (w, h) = layout.map_or((t.width, t.height), |l| {
        (
            i32::try_from(l.width).unwrap_or(i32::MAX),
            i32::try_from(l.height).unwrap_or(i32::MAX),
        )
    });
    if let Some(l) = layout
        && len < l.bytes
    {
        return Err(DecodeError::NotEnoughData {
            needed: l.bytes as u64,
            got: len as u64,
        });
    }
    let base = crate::plan(format, len, w, h)?;
    Ok((format, base, layout))
}

/// Platform-aware counterpart of [`crate::plan`]. Scratch includes the
/// transformed input, padded image, final crop and decoder tables while live.
pub fn plan_in(t: &TextureFields<'_>, data_len: usize) -> Result<Plan, DecodeError> {
    platform_plan(t, data_len, None)
}

/// Platform-aware planning retaining the decoder chosen by [`crate::select_in`].
/// In particular, Unity and legacy DXT Crunch share a numeric format value.
pub fn plan_in_as(
    format: Format,
    t: &TextureFields<'_>,
    data_len: usize,
) -> Result<Plan, DecodeError> {
    platform_plan(t, data_len, Some(format))
}

fn platform_plan(
    t: &TextureFields<'_>,
    data_len: usize,
    selected: Option<Format>,
) -> Result<Plan, DecodeError> {
    let (_, mut p, layout) = prepared(t, data_len, selected)?;
    if p.output_bytes == 0 {
        return Ok(p);
    }
    if t.platform == crate::XBOX360 && crate::XBOX_SWAP_FORMATS.contains(&t.texture_format)
        || layout.is_some()
    {
        p.scratch = p.scratch.saturating_add(heap_cost(data_len as u64));
    }
    if let Some(l) = layout {
        p.input_bytes = l.bytes as u64;
        let w = t.width.unsigned_abs();
        let h = t.height.unsigned_abs();
        if (w, h) != l.padded_size() {
            let shown = u64::from(w) * u64::from(h);
            if shown > MAX_CROP_PIXELS {
                return Err(DecodeError::DecompressionBomb {
                    pixels: shown,
                    max: MAX_CROP_PIXELS,
                });
            }
            p.scratch = p.scratch.saturating_add(p.charge());
            p.width = w;
            p.height = h;
            p.output_bytes = shown * p.mode.bytes_per_pixel();
        }
    }
    Ok(p)
}

/// Apply platform transforms before crunch/unpacking, then decode, crop and
/// flip as UnityPy does. Ordinary callers keep using [`crate::decode`].
pub fn decode_in(
    t: &TextureFields<'_>,
    data: &[u8],
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    platform_decode(t, data, meter, None)
}

/// Apply platform transforms while retaining a selected Crunch fork.
/// Switch RGB24/BGR24 promotion still chooses the four-channel decoder.
pub fn decode_in_as(
    format: Format,
    t: &TextureFields<'_>,
    data: &[u8],
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    platform_decode(t, data, meter, Some(format))
}

fn platform_decode(
    t: &TextureFields<'_>,
    data: &[u8],
    meter: &Arc<Meter>,
    selected: Option<Format>,
) -> Result<Image, DecodeError> {
    let p = platform_plan(t, data.len(), selected)?;
    let (format, base, layout) = prepared(t, data.len(), selected)?;
    if p.output_bytes == 0 {
        return crate::decode(format, data, 0, 0, meter);
    }
    let transformed;
    let input = if let Some(l) = layout {
        transformed = deswizzle(data, &l, meter)?;
        transformed.as_slice()
    } else if t.platform == crate::XBOX360 && crate::XBOX_SWAP_FORMATS.contains(&t.texture_format) {
        transformed = swap_bytes_for_xbox(data, meter)?;
        transformed.as_slice()
    } else {
        data
    };
    let image = crate::decode(
        format,
        input,
        i32::try_from(base.width).map_err(|_| PlatformError::SizeOverflow)?,
        i32::try_from(base.height).map_err(|_| PlatformError::SizeOverflow)?,
        meter,
    )?;
    if (base.width, base.height) == (p.width, p.height) {
        return Ok(image);
    }
    // The decoder already flipped the padded image. Its bottom h rows are
    // the top-left crop flipped, preserving crop-before-flip ordering.
    crate::image::crop_flipped(&image, p.width, p.height, meter)
}
