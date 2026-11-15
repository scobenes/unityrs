// Ported from Crunch Unity decoder in texture2ddecoder 1.0.5 (Zlib): Unity XOR selector and ETC palettes. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/unitycrunch/crn_decomp.h:3149-3299.
// Altered source: checked metered Rust, packed selectors instead of aliasing u16 arrays.
// Copyright (c) 2010-2016 Richard Geldreich, Jr. and Binomial LLC
// See LICENSE-UNITYCRUNCH.
use super::super::{
    CrunchError as Error, Result,
    header::{Header, Region},
    symbol::{Bits, Model},
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

pub(super) struct Palettes {
    pub colors: MeteredVec<u32>,
    pub selectors: MeteredVec<u32>,
    pub alphas: MeteredVec<u16>,
    pub alpha_selectors: MeteredVec<[u8; 6]>,
}
impl Palettes {
    pub fn new(header: &Header, data: &[u8], meter: &Arc<Meter>) -> Result<Self> {
        let [ce, cs, ae, ass] = header.palettes;
        let etc = header.format >= 10;
        Ok(Self {
            colors: if etc {
                colors(ce, data, meter)?
            } else {
                super::super::palettes::colors(ce, data, meter)?
            },
            selectors: selectors(cs, data, etc, meter)?,
            alphas: super::super::palettes::alphas(ae, data, meter)?,
            // decode_palettes gates both alpha palettes on endpoints.
            alpha_selectors: if ae.count != 0 {
                alpha_selectors(ass, data, etc, meter)?
            } else {
                MeteredVec::with_capacity(meter, 0)?
            },
        })
    }
}
fn colors(region: Region, data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u32>> {
    let mut out = MeteredVec::with_capacity(meter, region.count)?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let mut color = 0u32;
    for _ in 0..region.count {
        // Addition is on the entire packed word: carries propagate before
        // masking all four lanes (crn_decomp.h:3175-3181).
        for shift in (0..32).step_by(8) {
            color = color.wrapping_add(model.decode(&mut bits)?.wrapping_shl(shift));
        }
        color &= 0x1f1f_1f1f;
        out.push(color)?;
    }
    Ok(out)
}
fn selectors(
    region: Region,
    data: &[u8],
    etc: bool,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u32>> {
    let mut out = MeteredVec::with_capacity(meter, region.count * if etc { 2 } else { 1 })?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let mut s = 0u32;
    for _ in 0..region.count {
        for shift in (0..32).step_by(4) {
            s ^= model.decode(&mut bits)?.wrapping_shl(shift);
        }
        if etc {
            let selector = (!s & 0xaaaa_aaaa) | (!(s ^ (s >> 1)) & 0x5555_5555);
            let mut words = [0u32; 2];
            for h in 0..4 {
                for w in 0..4 {
                    let t = (8 + h + 4 * w) & 15;
                    for (flip, word) in words.iter_mut().enumerate() {
                        let shift = if flip == 0 {
                            w * 8 + h * 2
                        } else {
                            h * 8 + w * 2
                        };
                        let v = selector >> shift;
                        *word |= ((v >> 1 & 1) | (v & 1) << 16) << t;
                    }
                }
            }
            for word in words {
                out.push(word)?;
            }
        } else {
            out.push(((s ^ s << 1) & 0xaaaa_aaaa) | (s >> 1 & 0x5555_5555))?;
        }
    }
    Ok(out)
}
#[allow(clippy::cast_possible_truncation)] // Reference XOR assignment into uint8 truncates.
fn alpha_selectors(
    region: Region,
    data: &[u8],
    etc: bool,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<[u8; 6]>> {
    let mut out = MeteredVec::with_capacity(meter, region.count * if etc { 2 } else { 1 })?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let mut linear = [0u8; 8];
    let mut dxt = [0u32; 2];
    for _ in 0..region.count {
        let mut words = [0u64; 2];
        if etc {
            for (pair, state) in linear.iter_mut().enumerate() {
                // Reference stores XOR into uint8, intentionally truncating.
                *state ^= model.decode(&mut bits)? as u8;
                for half in 0..2 {
                    let p = pair * 2 + half;
                    let mut s = (*state >> (half * 3)) & 7;
                    if s <= 3 {
                        s = 3 - s;
                    }
                    words[0] |= u64::from(s) << (45 - 3 * p);
                    words[1] |= u64::from(s) << (45 - 3 * (4 * (p % 4) + p / 4));
                }
            }
            for word in words {
                let bytes = word.to_be_bytes();
                out.push(
                    bytes
                        .get(2..)
                        .ok_or(Error::Palette)?
                        .try_into()
                        .map_err(|_| Error::Palette)?,
                )?;
            }
        } else {
            const MAP: [u64; 8] = [0, 2, 3, 4, 5, 6, 7, 1];
            for (group, state) in dxt.iter_mut().enumerate() {
                for shift in (0..24).step_by(6) {
                    *state ^= model.decode(&mut bits)?.wrapping_shl(shift);
                }
                for p in 0..8 {
                    words[0] |= *MAP
                        .get(((*state >> (3 * p)) & 7) as usize)
                        .ok_or(Error::Palette)?
                        << (24 * group + 3 * p);
                }
            }
            let bytes = words[0].to_le_bytes();
            out.push(
                bytes
                    .get(..6)
                    .ok_or(Error::Palette)?
                    .try_into()
                    .map_err(|_| Error::Palette)?,
            )?;
        }
    }
    Ok(out)
}
