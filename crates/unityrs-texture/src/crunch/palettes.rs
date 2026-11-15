// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/crunch/crn_decomp.h:3354-3358, crn_decomp.h:3824-4115.
// Altered source: checked Rust palette delta decoding; alpha selectors use u64.
// Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.
// See LICENSE-CRUNCH and THIRD_PARTY_LICENSES.

use super::{
    CrunchError as Error, Result,
    header::{Header, Region},
    symbol::{Bits, Model},
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

pub(super) struct Palettes {
    pub colors: MeteredVec<u32>,
    pub color_selectors: MeteredVec<u32>,
    pub alphas: MeteredVec<u16>,
    pub alpha_selectors: MeteredVec<u64>,
}

impl Palettes {
    pub fn new(header: &Header, data: &[u8], meter: &Arc<Meter>) -> Result<Self> {
        let [ce, cs, ae, ass] = header.palettes;
        Ok(Self {
            colors: colors(ce, data, meter)?,
            color_selectors: color_selectors(cs, data, meter)?,
            alphas: alphas(ae, data, meter)?,
            alpha_selectors: alpha_selectors(ass, data, meter)?,
        })
    }
}

// Six components accumulate independently, modulo RGB565's 32/64/32,
// crn_decomp.h:3841-3902.
#[allow(clippy::many_single_char_names)] // RGB565 component names from the source.
fn colors(region: Region, data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u32>> {
    let mut out = MeteredVec::with_capacity(meter, region.count)?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let rb = Model::receive(&mut bits, meter)?;
    let g = Model::receive(&mut bits, meter)?;
    let mut previous = [0u32; 6];
    for _ in 0..region.count {
        for (i, value) in previous.iter_mut().enumerate() {
            let green = i == 1 || i == 4;
            *value = (*value
                + if green {
                    g.decode(&mut bits)?
                } else {
                    rb.decode(&mut bits)?
                })
                & if green { 63 } else { 31 };
        }
        let [a, b, c, d, e, f] = previous;
        out.push(c | (b << 5) | (a << 11) | (f << 16) | (e << 21) | (d << 27))?;
    }
    Ok(out)
}

fn alphas(region: Region, data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u16>> {
    let mut out = MeteredVec::with_capacity(meter, region.count)?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let (mut a, mut b) = (0, 0);
    // crn_decomp.h:3995-4030: two modulo-256 endpoint components.
    for _ in 0..region.count {
        a = (a + model.decode(&mut bits)?) & 255;
        b = (b + model.decode(&mut bits)?) & 255;
        out.push(u16::try_from(a | (b << 8)).map_err(|_| Error::Palette)?)?;
    }
    Ok(out)
}

// Each symbol describes two signed deltas in row-major (delta1,delta0)
// order. State persists across palette entries, modulo 4/8, then converts
// linear selectors to DXT ordering (crn_decomp.h:3904-3993, :4033-4115).
fn selectors<const N: usize>(
    bits: &mut Bits<'_>,
    model: &Model,
    current: &mut [u32; 16],
    map: &[u8; N],
    width: u8,
) -> Result<u64> {
    let max = u32::try_from(N).map_err(|_| Error::Palette)? - 1;
    let span = 2 * max + 1;
    for pair in current.as_chunks_mut::<2>().0 {
        let symbol = model.decode(bits)?;
        if symbol >= span * span {
            return Err(Error::Palette);
        }
        let [a, b] = pair;
        // Adding max+1 before subtracting max implements the signed
        // wrap without signed arithmetic or debug overflow.
        *a = (*a + symbol % span + (max + 1) - max) & max;
        *b = (*b + symbol / span + (max + 1) - max) & max;
    }
    let mut packed = 0u64;
    for (i, &value) in current.iter().enumerate() {
        let selector = *map.get(value as usize).ok_or(Error::Palette)?;
        packed |= u64::from(selector) << (i * usize::from(width));
    }
    Ok(packed)
}

fn color_selectors(region: Region, data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u32>> {
    let mut out = MeteredVec::with_capacity(meter, region.count)?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let mut current = [0; 16];
    for _ in 0..region.count {
        out.push(
            u32::try_from(selectors(
                &mut bits,
                &model,
                &mut current,
                &[0, 2, 3, 1],
                2,
            )?)
            .map_err(|_| Error::Palette)?,
        )?;
    }
    Ok(out)
}

fn alpha_selectors(region: Region, data: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u64>> {
    let mut out = MeteredVec::with_capacity(meter, region.count)?;
    if region.count == 0 {
        return Ok(out);
    }
    let mut bits = Bits::new(region.bytes(data)?)?;
    let model = Model::receive(&mut bits, meter)?;
    let mut current = [0; 16];
    for _ in 0..region.count {
        out.push(selectors(
            &mut bits,
            &model,
            &mut current,
            &[0, 2, 3, 4, 5, 6, 7, 1],
            3,
        )?)?;
    }
    Ok(out)
}
