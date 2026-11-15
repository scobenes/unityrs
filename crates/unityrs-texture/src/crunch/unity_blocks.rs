// Ported from Crunch Unity decoder in texture2ddecoder 1.0.5 (Zlib): DXT and ETC reference traversal. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/unitycrunch/crn_decomp.h:3092-3129, :3352-3464, :3581-3725.
// Altered source: checked slices, metered row state, no pointer arithmetic.
// Copyright (c) 2010-2016 Richard Geldreich, Jr. and Binomial LLC
// See LICENSE-UNITYCRUNCH.
use super::{
    super::{
        CrunchError as Error, Result,
        header::Header,
        symbol::{Bits, Model},
    },
    palettes::Palettes,
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

pub(super) struct Models {
    reference: Model,
    color: Model,
    selector: Model,
    alpha: Option<(Model, Model)>,
}
impl Models {
    pub fn new(header: &Header, data: &[u8], meter: &Arc<Meter>) -> Result<Self> {
        let mut bits = Bits::new(header.tables.bytes(data)?)?;
        Ok(Self {
            reference: Model::receive(&mut bits, meter)?,
            color: Model::receive(&mut bits, meter)?,
            selector: Model::receive(&mut bits, meter)?,
            alpha: if header.palettes[2].count != 0 {
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
#[derive(Clone, Copy, Default)]
struct Row {
    reference: u16,
    color: u16,
    alpha: u16,
    initialized: u16,
}
fn advance(bits: &mut Bits<'_>, model: &Model, index: &mut usize, count: usize) -> Result<()> {
    let sum = u32::try_from(*index)
        .map_err(|_| Error::Palette)?
        .wrapping_add(model.decode(bits)?);
    let count = u32::try_from(count).map_err(|_| Error::Palette)?;
    *index = usize::try_from(if sum >= count { sum - count } else { sum })
        .map_err(|_| Error::Palette)?;
    Ok(())
}
fn get<T: Copy>(values: &MeteredVec<T>, index: usize) -> Result<T> {
    values.as_slice().get(index).copied().ok_or(Error::Palette)
}
fn row_mut(row: &mut MeteredVec<Row>, index: usize) -> Result<&mut Row> {
    row.as_mut_slice().get_mut(index).ok_or(Error::Palette)
}
fn put(out: &mut [u8], at: usize, bytes: &[u8]) -> Result<()> {
    out.get_mut(at..at + bytes.len())
        .ok_or(Error::Palette)?
        .copy_from_slice(bytes);
    Ok(())
}
#[allow(clippy::cast_possible_truncation)] // flip is one bit.
fn etc_endpoint(a: u32, b: u32, flip: usize) -> [u8; 4] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let diff = a
        .iter()
        .zip(b.iter())
        .take(3)
        .all(|(&x, &y)| i16::from(y) - i16::from(x) >= -4 && i16::from(y) - i16::from(x) <= 3);
    let mut out = [0u8; 4];
    for ((value, &x), &y) in out.iter_mut().zip(a.iter()).zip(b.iter()).take(3) {
        *value = if diff {
            x.wrapping_shl(3) | y.wrapping_sub(x) & 7
        } else {
            (x.wrapping_shl(3) & 0xf0) | y >> 1
        };
    }
    out[3] = a[3].wrapping_shl(5) | b[3].wrapping_shl(2) | u8::from(diff) << 1 | flip as u8;
    out
}

#[allow(clippy::cast_possible_truncation)] // Reference uint8 group and uint16 row stores truncate.
#[allow(clippy::too_many_lines)] // Keep reference traversal and visibility in a single checked loop.
pub(super) fn unpack(
    source: &[u8],
    size: (usize, usize),
    kind: (bool, bool),
    models: &Models,
    palettes: &Palettes,
    out: &mut [u8],
    meter: &Arc<Meter>,
) -> Result<()> {
    let (etc, alpha) = kind;
    let (bx, by) = (size.0.div_ceil(4), size.1.div_ceil(4));
    let (width, height) = ((bx + 1) & !1, (by + 1) & !1);
    let stride = if etc { 2 } else { 1 };
    let mut row = MeteredVec::with_capacity(meter, width * stride)?;
    for _ in 0..width * stride {
        row.push(Row::default())?;
    }
    let mut bits = Bits::new(source)?;
    let (mut ce, mut ae) = (0, 0);
    let mut diagonal: Option<(usize, usize)> = Some((0, 0));
    let mut group = 0u8;
    for y in 0..height {
        for x in 0..width {
            // DXT groups one 8-bit reference symbol per 2x2 blocks. ETC groups
            // one per vertical pair, with subblock mode bits interleaved.
            if y & 1 == 0 && (etc || x & 1 == 0) {
                group = models.reference.decode(&mut bits)? as u8;
            }
            let buffer = row_mut(&mut row, x * stride)?;
            let mut reference = if y & 1 != 0 {
                buffer.reference as u8
            } else if etc {
                buffer.reference = u16::from((group >> 2 & 3) | (group >> 4 & 0x0c));
                (group & 3) | (group >> 2 & 0x0c)
            } else {
                let reference = group & 3;
                group >>= 2;
                buffer.reference = u16::from(group & 3);
                group >>= 2;
                reference
            };
            match reference & 3 {
                0 => {
                    advance(&mut bits, &models.color, &mut ce, palettes.colors.len())?;
                    if alpha {
                        advance(
                            &mut bits,
                            &models.alpha.as_ref().ok_or(Error::Palette)?.0,
                            &mut ae,
                            palettes.alphas.len(),
                        )?;
                    }
                    buffer.color = ce as u16;
                    buffer.alpha = ae as u16;
                    buffer.initialized = 1;
                }
                1 => {
                    buffer.color = ce as u16;
                    buffer.alpha = ae as u16;
                    buffer.initialized = 1;
                }
                3 if etc => {
                    (ce, ae) = diagonal.ok_or(Error::Reference)?;
                    buffer.color = ce as u16;
                    buffer.alpha = ae as u16;
                    buffer.initialized = 1;
                }
                _ => {
                    if buffer.initialized == 0 {
                        return Err(Error::Reference);
                    }
                    ce = usize::from(buffer.color);
                    ae = usize::from(buffer.alpha);
                }
            }
            // Unlike DXT, ETC dereferences both color endpoints even outside
            // the image (crn_decomp.h:3621-3639, :3695-3719).
            let e0 = if etc { get(&palettes.colors, ce)? } else { 0 };
            let cs = models.selector.decode(&mut bits)? as usize;
            let ass = if alpha {
                models
                    .alpha
                    .as_ref()
                    .ok_or(Error::Palette)?
                    .1
                    .decode(&mut bits)? as usize
            } else {
                0
            };
            let mut e1 = 0;
            if etc {
                reference >>= 2;
                if reference != 0 {
                    advance(&mut bits, &models.color, &mut ce, palettes.colors.len())?;
                }
                e1 = get(&palettes.colors, ce)?;
                let saved = row_mut(&mut row, x * stride + 1)?;
                diagonal = (saved.initialized != 0)
                    .then_some((usize::from(saved.color), usize::from(saved.alpha)));
                saved.color = ce as u16;
                saved.alpha = ae as u16;
                saved.initialized = 1;
            }
            if x >= bx || y >= by {
                continue;
            }
            let block_bytes = if alpha { 16 } else { 8 };
            let mut at = (y * bx + x) * block_bytes;
            let flip = if etc {
                usize::from((reference >> 1) ^ 1)
            } else {
                0
            };
            if alpha {
                put(out, at, &get(&palettes.alphas, ae)?.to_le_bytes())?;
                put(
                    out,
                    at + 2,
                    &get(
                        &palettes.alpha_selectors,
                        ass * if etc { 2 } else { 1 } + flip,
                    )?,
                )?;
                at += 8;
            }
            let endpoint = if etc {
                etc_endpoint(e0, e1, flip)
            } else {
                get(&palettes.colors, ce)?.to_le_bytes()
            };
            put(out, at, &endpoint)?;
            put(
                out,
                at + 4,
                &get(&palettes.selectors, cs * if etc { 2 } else { 1 } + flip)?.to_le_bytes(),
            )?;
        }
    }
    Ok(())
}
