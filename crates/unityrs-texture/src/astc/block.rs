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

use super::block_mode::WeightMode;
use super::context::Tables;
use super::decimation::{ENTRY, infill};
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

/// Whether astcenc reads `block` as an error block, the status
/// [`decode`] writes the error colour for.
pub(super) fn is_error(tables: &mut Tables, block: &[u8; 16]) -> bool {
    let mut out = [0; 4 * MAX_TEXELS];
    symbolic(tables, block, &mut out).is_none()
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
    let (plane1, plane2) = grid_weights(weights, &reversed, count)?;
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
        *e = endpoint::unpack_ldr(cem, v)?.map(|end| end.map(u32::from));
    }
    // astcenc_symbolic_physical.cpp:538-543: no channel for one plane.
    let plane2_channel = if dual {
        usize::try_from(read_bits(block, layout.below_weights.checked_sub(2)?, 2)?).ok()?
    } else {
        4
    };
    let partition = (layout.partitions > 1).then_some((layout.seed, layout.partitions));
    let fp = tables.footprint();
    let identity = (weights.x, weights.y) == (fp.width(), fp.height());
    let (grid, parts) = tables.rows((weights.x, weights.y), partition)?;
    let texels = (grid.as_chunks::<{ ENTRY }>().0.get(..texels)?, parts);
    let weights = (&plane1, &plane2, plane2_channel);
    match (identity, dual) {
        (true, false) => colours::<true, false>(texels, weights, &ends, out),
        (true, true) => colours::<true, true>(texels, weights, &ends, out),
        (false, false) => colours::<false, false>(texels, weights, &ends, out),
        (false, true) => colours::<false, true>(texels, weights, &ends, out),
    }
    Some(())
}

/// The unquantised weights of each plane's grid, read from the bit-reversed
/// block (astcenc_symbolic_physical.cpp:396-424): `count` in all, the
/// second plane's interleaved.
fn grid_weights(
    weights: WeightMode,
    reversed: &[u8; 16],
    count: usize,
) -> Option<([u8; 64], [u8; 64])> {
    let mut indices = [0; 64];
    ise::decode(weights.quant, reversed, 0, indices.get_mut(..count)?)?;
    let (mut plane1, mut plane2) = ([0; 64], [0; 64]);
    if weights.dual_plane {
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
    Some((plane1, plane2))
}

/// Each texel's colour (astcenc_decompress_symbolic.cpp:89-155 and
/// :157-310) from its weight, the first plane's in every channel but the
/// second plane's channel when there is one (`DUAL`), and its partition's
/// endpoints (`parts` empty for one partition). A grid of the footprint's
/// size (`IDENTITY`) gives each texel its own weight: its entry is that
/// weight's index alone at 16 sixteenths, which `infill` rounds back to the
/// weight (`(8 + 16 w) >> 4`); such a grid has at most 64 texels.
///
/// Each of the four is its own function, compiled with its loop's
/// constant choices, the weights and interpolation inlined.
#[inline(never)]
fn colours<const IDENTITY: bool, const DUAL: bool>(
    (entries, parts): (&[[u8; ENTRY]], &[u8]),
    (plane1, plane2, channel): (&[u8; 64], &[u8; 64], usize),
    ends: &[[[u32; 4]; 2]; 4],
    out: &mut [u8; 4 * MAX_TEXELS],
) {
    let weight = |i: usize, entry: [u8; ENTRY], plane: &[u8; 64]| {
        if IDENTITY {
            plane.get(i & 63).copied().unwrap_or(0)
        } else {
            infill(entry, plane)
        }
    };
    let pixels = out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(entries)
        .enumerate();
    for (i, (px, &entry)) in pixels {
        let mut w = [weight(i, entry, plane1); 4];
        if DUAL {
            // A dual-plane block's channel is 2 bits.
            if let Some(w) = w.get_mut(channel & 3) {
                *w = weight(i, entry, plane2);
            }
        }
        // A partition is below 4: `partition::select`'s count is at most 4.
        let part = parts.get(i).map_or(0, |&p| usize::from(p & 3));
        let [e0, e1] = ends.get(part).copied().unwrap_or_default();
        *px = lerp(e0, e1, w);
    }
}

/// astcenc_decompress_symbolic.cpp:43-55 with every lane `decode_unorm8`:
/// each channel's interpolated 16-bit value's top byte, which the store
/// keeps.
#[allow(clippy::inline_always, reason = "measured: the texel loop's hot call")]
#[inline(always)]
fn lerp(c0: [u32; 4], c1: [u32; 4], w: [u8; 4]) -> [u8; 4] {
    #[allow(clippy::cast_possible_truncation, reason = "a 16-bit value's top byte")]
    let lane = |c0: u32, c1: u32, w: u8| {
        let (w1, w0) = (u32::from(w), 64 - u32::from(w.min(64)));
        // At most 65,535: `>> 6`, then the top byte's `>> 8`.
        ((c0 * w0 + c1 * w1 + 32) >> 14) as u8
    };
    [
        lane(c0[0], c1[0], w[0]),
        lane(c0[1], c1[1], w[1]),
        lane(c0[2], c1[2], w[2]),
        lane(c0[3], c1[3], w[3]),
    ]
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
