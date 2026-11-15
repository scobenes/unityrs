// Ported from vgmstream r2168 (ISC): src/coding/ima_decoder.c, std_ima_expand_nibble_data, decode_xbox_ima, decode_fsb_ima; src/coding/fadpcm_decoder.c, decode_fadpcm. See THIRD_PARTY_LICENSES.
//! Frame layouts from r2168 (d9875b1f). Edge semantics follow the measured
//! FMOD 2.00.22 Linux `x86_64` oracle in tests/fixtures/audio.json:
//! unsigned invalid IMA steps silence a frame/channel; FADPCM coefficients
//! use &7 and measured entries 5-7. Incomplete IMA blocks stay silent;
//! FADPCM decodes complete channel frames in an incomplete final block
//! only when the output frame has all 256 samples.
use crate::{Error, Subsound};
use std::sync::Arc;
use unityrs_core::limits::Meter;

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
// Entries 5-7 and &7 are measured from FMOD, not vgmstream's zero/%7 rule.
// Oracle cases fadpcm_coef_0 through fadpcm_coef_15 pin all entries/aliases.
const COEFS: [(i32, i32); 8] = [
    (0, 0),
    (60, 0),
    (122, 60),
    (115, 52),
    (98, 55),
    (70, 0),
    (77, 60),
    (79, 122),
];

pub(crate) fn layout(mode: u32, channels: u16) -> Option<(usize, usize)> {
    let (bytes, samples) = match mode {
        7 => (0x24, 64),
        16 => (0x8c, 256),
        _ => return None,
    };
    Some((bytes * usize::from(channels), samples))
}

pub(crate) fn decode(
    sub: &Subsound<'_>,
    mode: u32,
    output: &mut [u8],
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    let channels = usize::from(sub.channels);
    let (block, samples) = layout(mode, sub.channels).ok_or(Error::Unsupported { codec: mode })?;
    if block == 0 {
        return Err(Error::Invalid("channels"));
    }
    // Zero initialization is charged by the WAV writer. Frame work is charged
    // before expansion. IMA requires a whole multichannel block; FADPCM's
    // final block preserves complete channel frames only for a full output
    // frame. A partial output frame requires the whole compressed block.
    for (frame, out) in sub
        .data
        .chunks(block)
        .zip(output.chunks_mut(samples * channels * 2))
    {
        if frame.len() != block && (mode == 7 || out.len() != samples * channels * 2) {
            continue;
        }
        meter.count_decode((samples * channels) as u64)?;
        if mode == 7 {
            ima(frame, channels, out)?;
        } else {
            for (channel, frame) in frame.as_chunks::<0x8c>().0.iter().enumerate() {
                fadpcm(frame, channels, channel, out)?;
            }
        }
    }
    Ok(())
}

fn byte(frame: &[u8], at: usize) -> Result<u8, Error> {
    frame.get(at).copied().ok_or(Error::Invalid("ADPCM frame"))
}
fn history(frame: &[u8], at: usize) -> Result<i32, Error> {
    Ok(i32::from(i16::from_le_bytes([
        byte(frame, at)?,
        byte(frame, at + 1)?,
    ])))
}
fn put(
    out: &mut [u8],
    channels: usize,
    channel: usize,
    sample: usize,
    value: i32,
) -> Result<(), Error> {
    let at = (sample * channels + channel) * 2;
    let dest = out.get_mut(at..at + 2).ok_or(Error::OutputSize)?;
    let value = i16::try_from(value).map_err(|_| Error::Invalid("ADPCM sample"))?;
    dest.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn ima_step_offset(channels: usize, channel: usize) -> usize {
    if channels <= 2 {
        channel * 4 + 2
    } else {
        channels * 2 + channel * 2
    }
}
fn ima(frame: &[u8], channels: usize, out: &mut [u8]) -> Result<(), Error> {
    if channels <= 2 {
        for c in 0..channels {
            if byte(frame, ima_step_offset(channels, c))? > 88 {
                return Ok(());
            }
        }
    }
    let samples = out.len() / (channels * 2);
    for c in 0..channels {
        let mut index = i32::from(byte(frame, ima_step_offset(channels, c))?);
        if index > 88 {
            continue;
        }
        let mut hist = history(frame, c * if channels <= 2 { 4 } else { 2 })?;
        put(out, channels, c, 0, hist)?;
        for sample in 1..samples {
            let nibble = sample - 1;
            let interleave = if channels <= 2 { 4 } else { 2 };
            let group = nibble / (interleave * 2);
            let within = nibble % (interleave * 2);
            let at = 4 * channels + interleave * c + group * interleave * channels + within / 2;
            let code = (byte(frame, at)? >> ((within & 1) * 4)) & 15;
            let step = *STEPS
                .get(usize::try_from(index).map_err(|_| Error::Invalid("IMA index"))?)
                .ok_or(Error::Invalid("IMA index"))?;
            // Shift-and-add rounding is observable; multiplying differs by >1 LSB.
            let mut delta = step >> 3;
            if code & 1 != 0 {
                delta += step >> 2;
            }
            if code & 2 != 0 {
                delta += step >> 1;
            }
            if code & 4 != 0 {
                delta += step;
            }
            if code & 8 != 0 {
                delta = -delta;
            }
            hist = (hist + delta).clamp(-32768, 32767);
            index = (index
                + INDEX
                    .get(usize::from(code & 7))
                    .ok_or(Error::Invalid("IMA code"))?)
            .clamp(0, 88);
            put(out, channels, c, sample, hist)?;
        }
    }
    Ok(())
}
fn fadpcm(frame: &[u8], channels: usize, channel: usize, out: &mut [u8]) -> Result<(), Error> {
    let mut hist1 = history(frame, 8)?;
    let mut hist2 = history(frame, 10)?;
    for sample in 0..out.len() / (channels * 2) {
        let group = sample / 32;
        let shift = (group & 1) * 4;
        let coef = (byte(frame, group / 2)? >> shift) & 7;
        let scale = (byte(frame, 4 + group / 2)? >> shift) & 15;
        let &(c1, c2) = COEFS
            .get(usize::from(coef))
            .ok_or(Error::Invalid("FADPCM coefficient"))?;
        let nibble = (byte(frame, 12 + sample / 2)? >> ((sample & 1) * 4)) & 15;
        // Sign extend without a signed left-shift overflow, then scale.
        #[allow(clippy::cast_possible_wrap)]
        let residual = ((u32::from(nibble) << 28) as i32) >> (22 - scale);
        let value = ((residual - hist2 * c2 + hist1 * c1) >> 6).clamp(-32768, 32767);
        put(out, channels, channel, sample, value)?;
        hist2 = hist1;
        hist1 = value;
    }
    Ok(())
}
