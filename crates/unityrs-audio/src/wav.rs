// Ported from fmod_toolkit 0.1.3 (MIT): fmod.py's integer/float WAV writer and numpy conversion. See THIRD_PARTY_LICENSES.
//! `fmod.py` lines 101-192 define WAV fields, 195-206 the float conversion.
//! The pinned oracle has numpy: multiply in float32, clip, truncate to i16.
//! NaN converts to zero; infinities saturate. The no-numpy branch's different
//! thresholds are outside the pinned stack. No FMOD code is used.
use crate::{Error, Subsound};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

pub(crate) fn width(mode: u32) -> Result<u8, Error> {
    match mode {
        1 => Ok(1),
        2 => Ok(2),
        3 => Ok(3),
        4 | 5 => Ok(4),
        codec => Err(Error::Unsupported { codec }),
    }
}
fn bits(mode: u32, convert: bool) -> Result<u16, Error> {
    Ok(if mode == 5 && convert {
        16
    } else {
        u16::from(width(mode)?) * 8
    })
}
pub(crate) fn size(sub: &Subsound<'_>, mode: u32, convert: bool) -> Result<usize, Error> {
    let bits = bits(mode, convert)?;
    let length = u64::from(sub.samples) * u64::from(sub.channels) * u64::from(bits / 8);
    let block = u32::from(sub.channels) * u32::from(bits / 8);
    if length > (i32::MAX - 36) as u64
        || block > i16::MAX as u32
        || u64::from(sub.frequency) * u64::from(block) > i32::MAX as u64
    {
        return Err(Error::OutputSize);
    }
    usize::try_from(length + 44).map_err(|_| Error::OutputSize)
}
pub(crate) fn decode(
    sub: &Subsound<'_>,
    mode: u32,
    convert: bool,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    let bits = bits(mode, convert)?;
    let length = u32::try_from(sub.wav_bytes.checked_sub(44).ok_or(Error::OutputSize)?)
        .map_err(|_| Error::OutputSize)?;
    let block = sub
        .channels
        .checked_mul(bits / 8)
        .ok_or(Error::OutputSize)?;
    let rate = sub
        .frequency
        .checked_mul(u32::from(block))
        .ok_or(Error::OutputSize)?;
    meter.count_decode((sub.data.len() as u64).max(sub.wav_bytes as u64))?;
    let mut out = MeteredVec::with_capacity(meter, sub.wav_bytes)?;
    out.extend_from_slice(b"RIFF")?;
    out.extend_from_slice(&(length + 36).to_le_bytes())?;
    out.extend_from_slice(b"WAVEfmt ")?;
    out.extend_from_slice(&16u32.to_le_bytes())?;
    let format = if mode == 5 && !convert { 3u16 } else { 1u16 };
    out.extend_from_slice(&format.to_le_bytes())?;
    out.extend_from_slice(&sub.channels.to_le_bytes())?;
    out.extend_from_slice(&sub.frequency.to_le_bytes())?;
    out.extend_from_slice(&rate.to_le_bytes())?;
    out.extend_from_slice(&block.to_le_bytes())?;
    out.extend_from_slice(&bits.to_le_bytes())?;
    out.extend_from_slice(b"data")?;
    out.extend_from_slice(&length.to_le_bytes())?;
    if mode == 5 && convert {
        for chunk in sub.data.as_chunks::<4>().0 {
            let value = f32::from_le_bytes(*chunk);
            #[allow(clippy::cast_possible_truncation)]
            // numpy truncates after float32 multiply and clip.
            let value = (value * 32768.0).clamp(-32768.0, 32767.0) as i16;
            out.extend_from_slice(&value.to_le_bytes())?;
        }
    } else {
        out.extend_from_slice(sub.data)?;
    }
    Ok(out)
}
