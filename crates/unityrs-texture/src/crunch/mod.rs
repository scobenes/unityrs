// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/crunch/crn_decomp.h. Altered source: safe Rust port.
// Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.
// See LICENSE-CRUNCH and THIRD_PARTY_LICENSES.
//! Legacy CRN unpacking, followed by the existing Pillow BC1/BC3 decoder.
//!
//! The sdist's `src/pylink.cpp:134-155` always requests level zero;
//! `src/Texture2DDecoder/crunch.cpp:6-40` supplies a single face pointer.
//! Cubemaps would overrun that pointer array and are refused here.
//! Checksums are deliberately ignored, as in that wrapper's calls to
//! `crnd_get_texture_info` and `crnd_unpack_begin`, rather than
//! `crnd_validate_file` (crn_decomp.h:2702-2771, crn_decomp.h:3667-3688).
//! All offsets, counts, code lengths and reads are checked before use.
//! Nonempty bit regions zero-extend on exhaustion, matching the reference.
//! Header ranges still have to name bytes in the supplied buffer.
//!
//! This legacy decoder contains static Huffman models only
//! (crn_decomp.h:1981-2042, crn_decomp.h:3085-3161). The changing state is
//! the chunk encoding reservoir and previous palette indices, not an
//! adaptive Huffman tree. Unity's fork is a separate, unsupported codec.

mod blocks;
mod header;
mod palettes;
mod symbol;

/// Why legacy crunch could not be unpacked. No error owns input-sized heap.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CrunchError {
    /// A malformed header, range, dimension or palette descriptor.
    #[error("invalid legacy crunch header: {0}")]
    Header(&'static str),
    /// A bitstream region is empty (reference `start_decoding` refuses it).
    #[error("truncated legacy crunch bitstream")]
    Truncated,
    /// Invalid Huffman model, run or canonical code.
    #[error("invalid legacy crunch Huffman code")]
    Huffman,
    /// A decoded palette index or selector delta is out of range.
    #[error("invalid legacy crunch palette index")]
    Palette,
    /// The reference `Texture2D` wrapper provides only one destination face.
    #[error("legacy crunch Texture2D requires one face, got {0}")]
    Faces(u8),
    /// Segmented CRN needs an external level source the wrapper cannot supply.
    #[error("segmented legacy crunch is unsupported")]
    Segmented,
    /// Allocation or meter refusal, with every live buffer released on return.
    #[error(transparent)]
    Charge(#[from] unityrs_core::limits::ChargeError),
}
const _: () = crate::send_sync::<CrunchError>();

type Result<T> = std::result::Result<T, CrunchError>;

use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec, heap_cost};

/// A length-only plan cannot read counts: bound every palette by the
/// header's 16-bit count, every model by its 14-bit alphabet, and
/// include the temporary code-length model/array while palettes decode.
/// Actual allocations use the checked header counts, never this maximum.
/// Eight sorted-symbol arrays cover five persistent models, two palette
/// models, and the nested code-length model; one temporary length array.
pub(crate) fn scratch(block_bytes: u64) -> u64 {
    heap_cost(block_bytes)
        + heap_cost(65535 * 4) * 2
        + heap_cost(65535 * 2)
        + heap_cost(65535 * 8)
        + heap_cost(16383 * 2) * 8
        + heap_cost(16383)
}

/// Unpack the requested mip. Public `Texture2D` decoding always calls this
/// with zero (texture2ddecoder 1.0.5 sdist src/pylink.cpp:134-155).
pub(crate) fn unpack(
    data: &[u8],
    dxt5: bool,
    size: (usize, usize),
    level: usize,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>> {
    const ZEROS: [u8; 4096] = [0; 4096];
    let (header, source, width, height) = prepare(data, dxt5, size, level)?;
    let len = width
        .div_ceil(4)
        .checked_mul(height.div_ceil(4))
        .and_then(|n| n.checked_mul(if dxt5 { 16 } else { 8 }))
        .ok_or(CrunchError::Header("block buffer overflow"))?;
    // Palette symbol decodes are independent of image dimensions and may
    // greatly exceed the output work (6/8/2/8 symbols per entry). Count
    // their u32 bytes before setup; model alphabet work is charged by Model itself.
    let palette_work = header
        .palettes
        .iter()
        .zip([6, 8, 2, 8])
        .map(|(region, symbols)| region.count as u64 * symbols * 4)
        .sum();
    meter.count_decode(palette_work)?;
    let models = blocks::Models::new(&header, data, meter)?;
    let palettes = palettes::Palettes::new(&header, data, meter)?;
    let mut out = MeteredVec::with_capacity(meter, len)?;
    while out.len() < len {
        let n = (len - out.len()).min(ZEROS.len());
        out.extend_from_slice(
            ZEROS
                .get(..n)
                .ok_or(CrunchError::Header("block fill size"))?,
        )?;
    }
    blocks::unpack(
        source,
        dxt5,
        width,
        height,
        &models,
        &palettes,
        out.as_mut_slice(),
    )?;
    Ok(out)
}

/// Validate the header and requested level before either image buffer is
/// allocated. There is no dimension-dependent compressed-size minimum:
/// for N chunks with ordinary 0..511 layout symbols the *consumed* bits
/// are at least ceil(N/3) + 5N (DXT1)
/// or ceil(N/3) + 10N (DXT5): one-bit chunk codes for groups of three,
/// one endpoint and four selectors per channel, including edge blocks
/// (crn_decomp.h:4196-4303, crn_decomp.h:4397-4470). But the reader can
/// supply arbitrarily many of those bits as zeros. A single byte with
/// singleton models decodes even a 16384-square image identically to the
/// reference. Only an empty region is refused, by `start_decoding`
/// (crn_decomp.h:3166-3180). Caller meters bound this inherent expansion.
pub(crate) fn validate(data: &[u8], dxt5: bool, size: (usize, usize)) -> Result<()> {
    prepare(data, dxt5, size, 0).map(|_| ())
}

fn prepare(
    data: &[u8],
    dxt5: bool,
    size: (usize, usize),
    level: usize,
) -> Result<(header::Header, &[u8], usize, usize)> {
    let header = header::Header::parse(data)?;
    let (source, width, height) = header.level(data, level)?;
    if header.dxt5 != dxt5 {
        return Err(CrunchError::Header("Texture2D/CRN format mismatch"));
    }
    // The wrapper sizes its buffer from the CRN header while Pillow reads
    // it using Texture2D dimensions. Require the same block extent so the
    // length-only plan bounds this second buffer. Sub-block differences
    // still decode/crop exactly as Pillow does. See PARITY.md section 19.3.
    if (width.div_ceil(4), height.div_ceil(4)) != (size.0.div_ceil(4), size.1.div_ceil(4)) {
        return Err(CrunchError::Header("Texture2D/CRN block extent mismatch"));
    }
    if source.is_empty() {
        return Err(CrunchError::Truncated);
    }
    Ok((header, source, width, height))
}

#[cfg(test)]
mod tests;
