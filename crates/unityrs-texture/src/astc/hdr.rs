// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_symbolic_physical.cpp, physical_to_symbolic; astcenc_decompress_symbolic.cpp, decompress_symbolic_block and lerp_color_int; astcenc_vecmathlib.h, lns_to_sf16 and unorm16_to_sf16. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! Full HDR profile into binary16, then PARITY section 17.1's byte rule.
//! Structural errors are refused; the LDR path retains its oracle error colour.

use super::{Tables, block::MAX_TEXELS, decimation, endpoint, hdr_endpoint, ise, read_bits};
use crate::HdrError;

/// `astcenc_vecmathlib.h`'s integer LNS approximation, saturated to finite FP16.
fn lns(value: u16) -> u16 {
    let m = value & 0x7ff;
    let mantissa = if m < 512 {
        m * 3
    } else if m < 1536 {
        m * 4 - 512
    } else {
        m * 5 - 2048
    };
    (((value >> 11) << 10) | (mantissa >> 3)).min(0x7bff)
}

/// astcenc's UNORM16 conversion truncates the normalized significand.
fn unorm(value: u16) -> u16 {
    if value == u16::MAX {
        return 0x3c00;
    }
    if value < 4 {
        return value << 8;
    }
    let leading = value.leading_zeros();
    let mantissa = (u32::from(value) << (leading + 1)) & 0xffff;
    u16::try_from((mantissa >> 6) | ((14 - leading) << 10)).unwrap_or(0)
}

fn void_colour(block: &[u8; 16], mode: u16) -> Option<[u16; 4]> {
    if read_bits(block, 10, 2)? != 3 {
        return None;
    }
    let extents = [
        read_bits(block, 12, 13)?,
        read_bits(block, 25, 13)?,
        read_bits(block, 38, 13)?,
        read_bits(block, 51, 13)?,
    ];
    let [low_s, high_s, low_t, high_t] = extents;
    if (low_s >= high_s || low_t >= high_t) && extents != [0x1fff; 4] {
        return None;
    }
    let mut colour = [0; 4];
    for (dst, bytes) in colour.iter_mut().zip(block.get(8..)?.as_chunks::<2>().0) {
        let value = u16::from_le_bytes(*bytes);
        *dst = if mode & 0x200 != 0 {
            // astcenc widens to FP32 before its FP16 store, quieting sNaN.
            if value & 0x7c00 == 0x7c00 && value & 0x3ff != 0 {
                value | 0x200
            } else {
                value
            }
        } else {
            unorm(value)
        };
    }
    Some(colour)
}

/// A block to binary16 RGBA, row-major. Tables are the same charged tables
/// used by LDR; no heap storage is needed per block.
pub(super) fn block(
    tables: &mut Tables,
    data: &[u8; 16],
    out: &mut [[u16; 4]; MAX_TEXELS],
) -> Option<()> {
    let texels = tables.footprint().texels();
    let mode = u16::from_le_bytes([data[0], data[1]]) & 0x7ff;
    if mode & 0x1ff == 0x1fc {
        let colour = void_colour(data, mode)?;
        out.get_mut(..texels)?.fill(colour);
        return Some(());
    }
    let weights = tables.mode(mode)?;
    let dual = weights.dual_plane;
    let count = usize::from(weights.x) * usize::from(weights.y) * if dual { 2 } else { 1 };
    let reversed = u128::from_le_bytes(*data).reverse_bits().to_le_bytes();
    let mut indices = [0; 64];
    ise::decode(weights.quant, &reversed, 0, indices.get_mut(..count)?)?;
    let (mut plane1, mut plane2) = ([0; 64], [0; 64]);
    if dual {
        for ((&[a, b], dst1), dst2) in indices
            .get(..count)?
            .as_chunks::<2>()
            .0
            .iter()
            .zip(&mut plane1)
            .zip(&mut plane2)
        {
            *dst1 = weights.quant.unquant_weight(a)?;
            *dst2 = weights.quant.unquant_weight(b)?;
        }
    } else {
        for (&index, dst) in indices.get(..count)?.iter().zip(&mut plane1) {
            *dst = weights.quant.unquant_weight(index)?;
        }
    }
    let layout = endpoint::decode_layout(data, weights)?;
    let values = endpoint::decode_values(data, &layout)?;
    let mut endpoints = [([[0; 4]; 2], [false; 4]); 4];
    for ((&mode, values), dst) in layout
        .modes
        .iter()
        .zip(&values)
        .zip(&mut endpoints)
        .take(usize::from(layout.partitions))
    {
        *dst = hdr_endpoint::unpack(mode, values)?;
    }
    let plane2_channel = if dual {
        usize::try_from(read_bits(data, layout.below_weights.checked_sub(2)?, 2)?).ok()?
    } else {
        4
    };
    let partition = (layout.partitions > 1).then_some((layout.seed, layout.partitions));
    let (grid, parts) = tables.rows((weights.x, weights.y), partition)?;
    for (i, (&entry, dst)) in grid
        .as_chunks::<{ decimation::ENTRY }>()
        .0
        .iter()
        .zip(out)
        .take(texels)
        .enumerate()
    {
        let first = decimation::infill(entry, &plane1);
        let second = if dual {
            decimation::infill(entry, &plane2)
        } else {
            first
        };
        let part = if parts.is_empty() {
            0
        } else {
            usize::from(*parts.get(i)?)
        };
        let ([lo, hi], hdr) = endpoints.get(part)?;
        for (c, (((&a, &b), &hdr), dst)) in lo.iter().zip(hi).zip(hdr).zip(dst).enumerate() {
            let w = u32::from(if c == plane2_channel { second } else { first });
            let interpolated =
                u16::try_from((u32::from(a) * (64 - w) + u32::from(b) * w + 32) >> 6).ok()?;
            *dst = if hdr {
                lns(interpolated)
            } else {
                unorm(interpolated)
            };
        }
    }
    Some(())
}

pub(crate) fn decode_image(
    tables: &mut Tables,
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
) -> Result<(), HdrError> {
    let fp = tables.footprint();
    let (bw, bh) = (usize::from(fp.width()), usize::from(fp.height()));
    let across = w.div_ceil(bw);
    let mut texels = [[0; 4]; MAX_TEXELS];
    for (i, bytes) in data
        .as_chunks::<16>()
        .0
        .iter()
        .take(across * h.div_ceil(bh))
        .enumerate()
    {
        block(tables, bytes, &mut texels).ok_or(HdrError::InvalidBlock)?;
        for (n, px) in texels.iter().take(bw * bh).enumerate() {
            let (x, y) = ((i % across) * bw + n % bw, (i / across) * bh + n / bw);
            if x < w && y < h {
                let at = ((h - 1 - y) * w + x) * 4;
                if let Some(dst) = out.get_mut(at..at + 4) {
                    for (dst, &half) in dst.iter_mut().zip(px) {
                        *dst = crate::tierb::half_byte(half);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "hdr_tests.rs"]
mod tests;
