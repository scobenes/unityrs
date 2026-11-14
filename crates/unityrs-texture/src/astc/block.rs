// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_symbolic_physical.cpp:291-544, physical_to_symbolic; astcenc_decompress_symbolic.cpp:37-56 and astcenc_decompress_symbolic.cpp:157-310, lerp_color_int and decompress_symbolic_block, for the LDR profile with decode_unorm8; astcenc_image.cpp:369-445, store_image_block's UNORM8 store, and its error colour as the AVX2 build's astcenc_vecmathlib_avx2_8.h:645-648 and astcenc_vecmathlib_avx2_8.h:687-700 make it. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! One 16-byte block to its texels, as astcenc decodes it with the LDR
//! profile into a `U8` image (`decode_unorm8`), which is how UnityPy calls
//! it (`PARITY.md`, `Texture2DConverter.get_astc_context`).
//!
//! - **Error blocks** (astcenc_decompress_symbolic.cpp:190-204) are NaN in
//!   every channel, which the `U8` store means to write as magenta,
//!   (255, 0, 255, 255) (astcenc_image.cpp:423-432), and does in
//!   astc-encoder-py's scalar, SSE2 and SSE4.1 builds. Its AVX2 build, which
//!   UnityPy loads on any x86-64 host with AVX2, the oracle's among them,
//!   writes white, (255, 255, 255, 255): its NaN test is an ordered compare,
//!   false for NaN (astcenc_vecmathlib_avx2_8.h:645-648), so the magenta is
//!   never selected, and `min(NaN, 1.0)` is 1.0 (`_mm256_min_ps` returns its
//!   second operand for a NaN, astcenc_vecmathlib_avx2_8.h:687-700), stored
//!   as 255. unityrs writes [`ERROR`], the oracle's white (`PARITY.md`
//!   section 19.3); its tests replay the magenta builds too. Error blocks
//!   are: an illegal or reserved block mode, a
//!   mode whose grid does not fit the footprint, more than 18 colour
//!   integers, a colour precision below `QUANT_6`, four partitions with two
//!   planes (astcenc_symbolic_physical.cpp:372-517), and a void-extent block
//!   whose reserved bits are not all set or whose extents are empty and not
//!   all ones (astcenc_symbolic_physical.cpp:322-347).
//! - **Void extent.** A UNORM16 constant colour keeps each channel's top
//!   byte (`asr<8>` then `* 257`, astcenc_decompress_symbolic.cpp:213-227);
//!   an FP16 one, the HDR void extent, is NaN in the LDR profile, so the
//!   error colour (astcenc_decompress_symbolic.cpp:229-243).
//! - **Weights** are read from the bit-reversed block
//!   (astcenc_symbolic_physical.cpp:396-424), the second plane's
//!   interleaved, then spread over the texels (the `decimation` module).
//! - **Colour.** Each partition's endpoints (the `endpoint` module; an HDR
//!   endpoint mode is magenta endpoints, not an error block) are
//!   interpolated per channel as `(c0 * (64 - w) + c1 * w + 32) >> 6`, the
//!   plane-two channel with the second plane's weight, and `decode_unorm8`
//!   keeps the top byte (astcenc_decompress_symbolic.cpp:43-55).
//! - **The store.** astcenc turns that byte, replicated to 16 bits, into a
//!   float through FP16 and back to a byte as
//!   `(int)(min(f, 1) * 255 + 0.5)` (astcenc_image.cpp:391-394); the byte
//!   comes back unchanged for each of the 256 values, fused multiply-add or
//!   not (`the_unorm8_store_keeps_the_byte`), so the decoder writes it
//!   directly.

use super::context::Tables;
use super::decimation::ENTRY;
use super::{endpoint, ise, read_bits};

/// An error block's colour in UnityPy's output on the oracle, astcenc's
/// AVX2 build: white (module docs).
pub(crate) const ERROR: [u8; 4] = [255; 4];

/// astcenc's error colour as its scalar, SSE2 and SSE4.1 builds write it
/// (astcenc_image.cpp:423-432): magenta.
#[cfg(test)]
pub(crate) const MAGENTA: [u8; 4] = [255, 0, 255, 255];

/// The most texels a block has: 12 x 12.
pub(super) const MAX_TEXELS: usize = 144;

/// Decodes `block` into the first `width x height` texels of `out`, RGBA,
/// row-major, an error block as `error` ([`ERROR`] but in tests).
pub(super) fn decode(
    tables: &mut Tables,
    block: &[u8; 16],
    out: &mut [u8; 4 * MAX_TEXELS],
    error: [u8; 4],
) {
    let texels = tables.footprint().texels();
    if symbolic(tables, block, out).is_none() {
        for px in out.as_chunks_mut::<4>().0.iter_mut().take(texels) {
            *px = error;
        }
    }
}

/// The block's texels, or `None` for an error block.
fn symbolic(tables: &mut Tables, block: &[u8; 16], out: &mut [u8; 4 * MAX_TEXELS]) -> Option<()> {
    let texels = tables.footprint().texels();
    let mode = u16::from_le_bytes([block[0], block[1]]) & 0x7ff;
    if mode & 0x1ff == 0x1fc {
        let colour = void_extent(block, mode)?;
        for px in out.as_chunks_mut::<4>().0.iter_mut().take(texels) {
            *px = colour;
        }
        return Some(());
    }
    let weights = tables.mode(mode)?;
    let dual = weights.dual_plane;
    let per_plane = usize::from(weights.x) * usize::from(weights.y);
    let count = per_plane * if dual { 2 } else { 1 };
    // astcenc_symbolic_physical.cpp:396-408: the weights run from the
    // block's top bit down.
    let reversed = u128::from_le_bytes(*block).reverse_bits().to_le_bytes();
    let mut indices = [0; 64];
    ise::decode(weights.quant, &reversed, 0, indices.get_mut(..count)?)?;
    let (mut plane1, mut plane2) = ([0; 64], [0; 64]);
    if dual {
        for ((&[first, second], a), b) in indices
            .get(..count)?
            .as_chunks::<2>()
            .0
            .iter()
            .zip(&mut plane1)
            .zip(&mut plane2)
        {
            *a = weights.quant.unquant_weight(first)?;
            *b = weights.quant.unquant_weight(second)?;
        }
    } else {
        for (&index, a) in indices.get(..count)?.iter().zip(&mut plane1) {
            *a = weights.quant.unquant_weight(index)?;
        }
    }
    let layout = endpoint::decode_layout(block, weights)?;
    let values = endpoint::decode_values(block, &layout)?;
    let mut ends = [[[0; 4]; 2]; 4];
    for ((&cem, v), e) in layout
        .modes
        .iter()
        .zip(&values)
        .zip(&mut ends)
        .take(usize::from(layout.partitions))
    {
        *e = endpoint::unpack_ldr(cem, v)?;
    }
    // astcenc_symbolic_physical.cpp:538-543: no channel for one plane.
    let plane2_channel = if dual {
        usize::try_from(read_bits(block, layout.below_weights.checked_sub(2)?, 2)?).ok()?
    } else {
        4
    };
    let partition = (layout.partitions > 1).then_some((layout.seed, layout.partitions));
    let (grid, parts) = tables.rows((weights.x, weights.y), partition)?;
    let entries = grid.as_chunks::<{ ENTRY }>().0.iter().take(texels);
    let pixels = out.as_chunks_mut::<4>().0.iter_mut();
    for (i, (&entry, px)) in entries.zip(pixels).enumerate() {
        let w1 = super::decimation::infill(entry, &plane1);
        let w2 = if dual {
            super::decimation::infill(entry, &plane2)
        } else {
            w1
        };
        let part = if parts.is_empty() { 0 } else { *parts.get(i)? };
        let [e0, e1] = ends.get(usize::from(part))?;
        for (c, ((&c0, &c1), dst)) in e0.iter().zip(e1).zip(px).enumerate() {
            *dst = lerp(c0, c1, if c == plane2_channel { w2 } else { w1 });
        }
    }
    Some(())
}

/// astcenc_decompress_symbolic.cpp:43-55 with every lane `decode_unorm8`:
/// the interpolated 16-bit value's top byte, which the store keeps.
#[inline]
fn lerp(c0: u16, c1: u16, w: u8) -> u8 {
    let (w1, w0) = (u32::from(w), 64 - u32::from(w.min(64)));
    let c = (u32::from(c0) * w0 + u32::from(c1) * w1 + 32) >> 6;
    u8::try_from(c >> 8).unwrap_or(u8::MAX)
}

/// A void-extent block's colour, or `None` where it is an error block or an
/// FP16 colour (astcenc_symbolic_physical.cpp:302-347,
/// astcenc_decompress_symbolic.cpp:206-243).
fn void_extent(block: &[u8; 16], mode: u16) -> Option<[u8; 4]> {
    if read_bits(block, 10, 2)? != 3 {
        return None;
    }
    let low_s = read_bits(block, 12, 8)? | (read_bits(block, 20, 5)? << 8);
    let high_s = read_bits(block, 25, 13)?;
    let low_t = read_bits(block, 38, 8)? | (read_bits(block, 46, 5)? << 8);
    let high_t = read_bits(block, 51, 13)?;
    let all_ones = [low_s, high_s, low_t, high_t].iter().all(|&v| v == 0x1fff);
    if (low_s >= high_s || low_t >= high_t) && !all_ones {
        return None;
    }
    if mode & 0x200 != 0 {
        return None;
    }
    // Each channel is a little-endian u16 from byte 8 (astcenc_symbolic_physical.cpp:317-320);
    // decode_unorm8 keeps its top byte.
    Some([block[9], block[11], block[13], block[15]])
}

#[cfg(test)]
mod tests;
