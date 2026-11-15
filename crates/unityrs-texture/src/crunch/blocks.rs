// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/crunch/crn_decomp.h:3640-3658, crn_decomp.h:3796-3821,
// crn_decomp.h:4135-4479. Altered source: shared checked DXT1/DXT5 loop.
// Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.
// See LICENSE-CRUNCH and THIRD_PARTY_LICENSES.

use super::{
    CrunchError as Error, Result,
    header::Header,
    palettes::Palettes,
    symbol::{Bits, Model},
};
use std::sync::Arc;
use unityrs_core::limits::Meter;

pub(super) struct Models {
    chunk: Model,
    color_endpoint: Model,
    color_selector: Model,
    alpha: Option<(Model, Model)>,
}

impl Models {
    pub fn new(header: &Header, data: &[u8], meter: &Arc<Meter>) -> Result<Self> {
        let mut bits = Bits::new(header.tables.bytes(data)?)?;
        Ok(Self {
            chunk: Model::receive(&mut bits, meter)?,
            color_endpoint: Model::receive(&mut bits, meter)?,
            color_selector: Model::receive(&mut bits, meter)?,
            alpha: if header.palettes.get(2).is_some_and(|p| p.count != 0) {
                Some((
                    Model::receive(&mut bits, meter)?,
                    Model::receive(&mut bits, meter)?,
                ))
            } else {
                None
            },
        })
    }
}

const TILES: [[usize; 4]; 8] = [
    [0, 0, 0, 0],
    [0, 0, 1, 1],
    [0, 1, 0, 1],
    [0, 0, 1, 2],
    [1, 2, 0, 0],
    [0, 1, 0, 2],
    [1, 0, 2, 0],
    [0, 1, 2, 3],
];
const COUNTS: [usize; 8] = [1, 2, 2, 3, 3, 3, 3, 4];

/// Reference `limit` subtracts once, not an arbitrary modulo
/// (crn_decomp.h:4135-4141). Skipped selectors may temporarily leave
/// the index outside the palette; validate only when fetching an entry.
fn next(bits: &mut Bits<'_>, model: &Model, previous: &mut usize, count: usize) -> Result<usize> {
    let delta = usize::try_from(model.decode(bits)?).map_err(|_| Error::Palette)?;
    let sum = *previous + delta;
    *previous = if sum >= count { sum - count } else { sum };
    Ok(*previous)
}

pub(super) fn unpack(
    source: &[u8],
    dxt5: bool,
    width: usize,
    height: usize,
    models: &Models,
    palettes: &Palettes,
    out: &mut [u8],
) -> Result<()> {
    let mut bits = Bits::new(source)?;
    let color_endpoints = palettes.colors.as_slice();
    let alpha_endpoints = palettes.alphas.as_slice();
    let (blocks_x, blocks_y) = (width.div_ceil(4), height.div_ceil(4));
    let (chunks_x, chunks_y) = (blocks_x.div_ceil(2), blocks_y.div_ceil(2));
    let block_bytes = if dxt5 { 16 } else { 8 };
    let mut encoding = 1;
    let (mut ce, mut cs, mut ae, mut ass) = (0, 0, 0, 0);
    // Serpentine chunk traversal. Palette state and the three-chunk
    // encoding reservoir persist across rows (crn_decomp.h:4147-4311).
    for y in 0..chunks_y {
        for col in 0..chunks_x {
            let x = if y & 1 != 0 { chunks_x - 1 - col } else { col };
            if encoding == 1 {
                encoding = models.chunk.decode(&mut bits)?;
                encoding |= 512;
            }
            let layout = (encoding & 7) as usize;
            encoding >>= 3;
            let tiles = TILES.get(layout).ok_or(Error::Huffman)?;
            let count = *COUNTS.get(layout).ok_or(Error::Huffman)?;
            let mut colors = [0u32; 4];
            let mut alphas = [0u16; 4];
            // DXT5 sends ALL alpha endpoints before color endpoints,
            // then interleaves alpha/color selectors per block.
            // crn_decomp.h:4397-4428.
            if dxt5 {
                let (model, _) = models.alpha.as_ref().ok_or(Error::Palette)?;
                for value in alphas.iter_mut().take(count) {
                    let index = next(&mut bits, model, &mut ae, alpha_endpoints.len())?;
                    *value = *alpha_endpoints.get(index).ok_or(Error::Palette)?;
                }
            }
            for value in colors.iter_mut().take(count) {
                let index = next(
                    &mut bits,
                    &models.color_endpoint,
                    &mut ce,
                    color_endpoints.len(),
                )?;
                *value = *color_endpoints.get(index).ok_or(Error::Palette)?;
            }
            // unpack_dxt1/unpack_dxt5 fetch all endpoints above, including
            // tiles used only outside the image (crn_decomp.h:4207-4217,
            // :4397-4412). Advance every selector index, but fetch entries
            // only after the visibility check (:4270-4303, :4429-4470).
            for (block, &tile) in tiles.iter().enumerate() {
                if dxt5 {
                    let (_, model) = models.alpha.as_ref().ok_or(Error::Palette)?;
                    next(&mut bits, model, &mut ass, palettes.alpha_selectors.len())?;
                }
                next(
                    &mut bits,
                    &models.color_selector,
                    &mut cs,
                    palettes.color_selectors.len(),
                )?;
                let (bx, by) = (2 * x + block % 2, 2 * y + block / 2);
                if bx >= blocks_x || by >= blocks_y {
                    continue;
                }
                let alpha_selectors = if dxt5 {
                    *palettes
                        .alpha_selectors
                        .as_slice()
                        .get(ass)
                        .ok_or(Error::Palette)?
                } else {
                    0
                };
                let color_selectors = *palettes
                    .color_selectors
                    .as_slice()
                    .get(cs)
                    .ok_or(Error::Palette)?;
                let at = (by * blocks_x + bx) * block_bytes;
                let target = out.get_mut(at..at + block_bytes).ok_or(Error::Palette)?;
                let color_at = if dxt5 {
                    target
                        .get_mut(..2)
                        .ok_or(Error::Palette)?
                        .copy_from_slice(&alphas.get(tile).ok_or(Error::Palette)?.to_le_bytes());
                    target.get_mut(2..8).ok_or(Error::Palette)?.copy_from_slice(
                        alpha_selectors
                            .to_le_bytes()
                            .get(..6)
                            .ok_or(Error::Palette)?,
                    );
                    8
                } else {
                    0
                };
                target
                    .get_mut(color_at..color_at + 4)
                    .ok_or(Error::Palette)?
                    .copy_from_slice(&colors.get(tile).ok_or(Error::Palette)?.to_le_bytes());
                target
                    .get_mut(color_at + 4..color_at + 8)
                    .ok_or(Error::Palette)?
                    .copy_from_slice(&color_selectors.to_le_bytes());
            }
        }
    }
    Ok(())
}
