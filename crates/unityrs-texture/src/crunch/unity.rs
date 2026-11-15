// Ported from Crunch Unity decoder in texture2ddecoder 1.0.5 (Zlib): checked Unity CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/unitycrunch/crn_decomp.h:2967-3334, crn_defs.h:244-291,
// crnlib.h:48-81. Altered source: safe, metered Rust; one-face Texture2D only.
// Copyright (c) 2010-2016 Richard Geldreich, Jr. and Binomial LLC
// See LICENSE-UNITYCRUNCH.
//! Unity's fork shares the packed header, bit reader, static Huffman models,
//! DXT color endpoints and alpha endpoints with legacy Crunch. Selector
//! palettes and block reference traversal differ. ETC2 (CRN 11) uses the
//! ETC1 unpacker, as in the fork; only the requested `TextureFormat`'s family
//! is accepted. The ETC1S/ETC2AS and other CRN formats are outside this API.

#[path = "unity_blocks.rs"]
mod blocks;
#[path = "unity_palettes.rs"]
mod palettes;

use super::{CrunchError as Error, Result, header::Header};
use crate::Format;
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec, heap_cost};

pub(crate) fn scratch(block_bytes: u64, width: u64) -> u64 {
    // Five persistent models, two palette models, nested code-length model.
    // ETC selectors need both orientations; row state has two 8-byte
    // elements per even-rounded block column (crn_decomp.h:3094-3100).
    heap_cost(block_bytes)
        + heap_cost(65535 * 4)
        + heap_cost(65535 * 8)
        + heap_cost(65535 * 2)
        + heap_cost(65535 * 12)
        + heap_cost(16383 * 2) * 8
        + heap_cost(16383)
        + heap_cost(width.div_ceil(4).div_ceil(2) * 2 * 16)
}

fn prepare(
    data: &[u8],
    format: Format,
    size: (usize, usize),
    level: usize,
) -> Result<(Header, &[u8], usize, usize)> {
    let header = Header::parse_unity(data)?;
    let matches = match format {
        Format::UnityDxt1Crunched => header.format == 0,
        Format::UnityDxt5Crunched => header.format == 2,
        Format::EtcRgb4Crunched => matches!(header.format, 10 | 11),
        Format::Etc2Rgba8Crunched => header.format == 12,
        _ => false,
    };
    if !matches {
        return Err(Error::Header("Texture2D/CRN format mismatch"));
    }
    let (source, width, height) = header.level(data, level)?;
    if (width.div_ceil(4), height.div_ceil(4)) != (size.0.div_ceil(4), size.1.div_ceil(4)) {
        return Err(Error::Header("Texture2D/CRN block extent mismatch"));
    }
    if source.is_empty() {
        return Err(Error::Truncated);
    }
    Ok((header, source, width, height))
}

pub(crate) fn validate(data: &[u8], format: Format, size: (usize, usize)) -> Result<()> {
    prepare(data, format, size, 0).map(|_| ())
}

pub(crate) fn unpack(
    data: &[u8],
    format: Format,
    size: (usize, usize),
    level: usize,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>> {
    const ZERO: [u8; 4096] = [0; 4096];
    let (header, source, width, height) = prepare(data, format, size, level)?;
    let etc = header.format >= 10;
    let alpha = matches!(header.format, 2 | 12);
    let len = width
        .div_ceil(4)
        .checked_mul(height.div_ceil(4))
        .and_then(|n| n.checked_mul(if alpha { 16 } else { 8 }))
        .ok_or(Error::Header("block buffer overflow"))?;
    // Palette expansion is independent of output dimensions. Models charge
    // their own alphabet work, including nested length expansion.
    // Reference + color endpoint + selector, plus the alpha pair and
    // ETC's second color endpoint. Hidden padded blocks consume symbols too.
    super::block_work(
        (width, height),
        3 + u64::from(alpha) * 2 + u64::from(etc),
        meter,
    )?;
    super::palette_work(&header, [if etc { 4 } else { 6 }, 8, 2, 8], meter)?;
    let models = blocks::Models::new(&header, data, meter)?;
    let palettes = palettes::Palettes::new(&header, data, meter)?;
    let mut out = MeteredVec::with_capacity(meter, len)?;
    while out.len() < len {
        let n = (len - out.len()).min(ZERO.len());
        out.extend_from_slice(ZERO.get(..n).ok_or(Error::Palette)?)?;
    }
    blocks::unpack(
        source,
        (width, height),
        (etc, alpha),
        &models,
        &palettes,
        out.as_mut_slice(),
        meter,
    )?;
    Ok(out)
}

#[cfg(test)]
#[path = "unity_tests.rs"]
mod tests;
