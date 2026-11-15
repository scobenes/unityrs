//! FSB5 format facts, independently implemented with bounded readers.
//! Bit layout: Fmod5Sharp/FmodTypes/FmodSampleMetadata.cs in
//! <https://github.com/SamboyCoding/Fmod5Sharp> (bits 5-6 encode 1/2/6/8
//! channels; bits 7-33 encode 32-byte offsets). The rate at index 3 is
//! 12000 in the pinned FMOD oracle (tests/fixtures/audio.json), matching
//! that project. No third-party parser code is included.
use crate::{Error, send_sync, wav};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// Bank section lengths are checked against the actual slice before allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub subsound_count: u32,
    pub header_size: usize,
    pub sample_headers_size: u32,
    pub name_table_size: u32,
    pub data_size: u32,
    pub mode: u32,
}
const _: () = send_sync::<Header>();
/// A validated subsound. Bank names borrow bytes (no lossy UTF-8 decoding).
/// Unknown extra chunks remain available verbatim for future codecs.
#[derive(Debug)]
pub struct Subsound<'a> {
    pub name: Option<&'a [u8]>,
    pub frequency: u32,
    pub channels: u16,
    pub data_offset: u32,
    pub samples: u32,
    pub extra_chunks: &'a [u8],
    pub(crate) data: &'a [u8],
    pub(crate) wav_bytes: usize,
}
const _: () = send_sync::<Subsound<'static>>();

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    field: &'static str,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let out = span(self.bytes, self.pos, n, self.field)?;
        self.pos = self.pos.checked_add(n).ok_or(Error::OutputSize)?;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        word(self.take(4)?)
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| Error::Invalid("sample header"))?,
        ))
    }
}
fn word(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| Error::Invalid("32-bit field"))?,
    ))
}
fn span<'a>(bytes: &'a [u8], pos: usize, n: usize, field: &'static str) -> Result<&'a [u8], Error> {
    let end = pos.checked_add(n).ok_or(Error::OutputSize)?;
    bytes.get(pos..end).ok_or(Error::Truncated {
        field,
        offset: pos,
        length: n,
    })
}
fn rate(index: u64) -> Option<u32> {
    match index {
        0 => Some(4000),
        1 => Some(8000),
        2 => Some(11000),
        3 => Some(12000),
        4 => Some(16000),
        5 => Some(22050),
        6 => Some(24000),
        7 => Some(32000),
        8 => Some(44100),
        9 => Some(48000),
        10 => Some(96000),
        _ => None,
    }
}
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "FMOD returns float32; Python int truncates toward zero"
)]
fn wav_frequency(frequency: u32) -> Result<u32, Error> {
    // pyfmodex 0.7.2 sound.py:169-184 reads GetDefaults through c_float.
    // fmod_toolkit 0.1.3 fmod.py:117 then calls int(default_frequency),
    // before packing signed frequency/byte-rate fields at :156-168.
    // Widen the integer result so even u32::MAX rounding up cannot saturate.
    let frequency = (frequency as f32) as u64;
    u32::try_from(frequency)
        .ok()
        .filter(|&n| n > 0 && i32::try_from(n).is_ok())
        .ok_or(Error::Invalid("frequency"))
}
type Parsed<'a> = (Header, MeteredVec<Subsound<'a>>, u64);
#[allow(
    clippy::too_many_lines,
    reason = "one bounded traversal of bank sections and subsounds"
)]
pub(crate) fn parse<'a>(
    bytes: &'a [u8],
    convert: bool,
    meter: &Arc<Meter>,
) -> Result<Parsed<'a>, Error> {
    let mut r = Reader {
        bytes,
        pos: 4,
        field: "bank header",
    };
    let version = r.u32()?;
    let header_size = match version {
        0 => 64,
        1 => 60,
        n => return Err(Error::Version(n)),
    };
    let header = Header {
        version,
        header_size,
        subsound_count: r.u32()?,
        sample_headers_size: r.u32()?,
        name_table_size: r.u32()?,
        data_size: r.u32()?,
        mode: r.u32()?,
    };
    span(bytes, 0, header_size, "bank header")?;
    let mut sections = Reader {
        bytes,
        pos: header_size,
        field: "sample header section",
    };
    let sample_bytes = sections.take(header.sample_headers_size as usize)?;
    sections.field = "name table section";
    let names = sections.take(header.name_table_size as usize)?;
    sections.field = "data section";
    let data = sections.take(header.data_size as usize)?;
    // Trailing bytes are permitted, but no declared section may run into them.
    let count = header.subsound_count as usize;
    if count == 0 {
        return Err(Error::Invalid("zero subsound count"));
    }
    if count > sample_bytes.len() / 8 {
        return Err(Error::Invalid("subsound count"));
    }
    // Charge the metadata traversal before any allocation or loop. Names may
    // alias, so their per-byte searches are additionally counted below.
    meter.count_decode((header_size + sample_bytes.len() + names.len()) as u64)?;
    // Grow only after validating each record; the declared count is untrusted.
    let mut subs = MeteredVec::new(meter);
    let mut samples = Reader {
        bytes: sample_bytes,
        pos: 0,
        field: "sample header",
    };
    if !names.is_empty() && count > names.len() / 4 {
        return Err(Error::Invalid("name offsets"));
    }
    let mut name_offsets = Reader {
        bytes: names,
        pos: 0,
        field: "name offsets",
    };
    for _ in 0..count {
        let raw = samples.u64()?;
        let mut frequency = rate((raw >> 1) & 15);
        let mut channels = match (raw >> 5) & 3 {
            0 => 1,
            1 => 2,
            2 => 6,
            _ => 8,
        };
        let chunks_start = samples.pos;
        let mut more = raw & 1 != 0;
        while more {
            let chunk = samples.u32()?;
            more = chunk & 1 != 0;
            let size = ((chunk >> 1) & 0x00ff_ffff) as usize;
            let payload = samples.take(size)?;
            match chunk >> 25 {
                1 => {
                    if payload.len() != 1 {
                        return Err(Error::Invalid("channels chunk size"));
                    }
                    channels = u16::from(*payload.first().ok_or(Error::Invalid("channels chunk"))?);
                }
                2 => {
                    frequency = Some(word(payload)?);
                }
                _ => {}
            }
        }
        let frequency = wav_frequency(frequency.ok_or(Error::Invalid("frequency"))?)?;
        if channels == 0 {
            return Err(Error::Invalid("channels"));
        }
        let name = if names.is_empty() {
            None
        } else {
            let offset = name_offsets.u32()? as usize;
            if offset < count * 4 {
                return Err(Error::Invalid("name offset overlaps offsets"));
            }
            let tail = names.get(offset..).ok_or(Error::Invalid("name offset"))?;
            let mut length = 0;
            for &b in tail {
                meter.count_decode(1)?;
                if b == 0 {
                    break;
                }
                length += 1;
            }
            if length == tail.len() {
                return Err(Error::Invalid("unterminated name"));
            }
            Some(span(tail, 0, length, "name")?)
        };
        subs.push(Subsound {
            name,
            frequency,
            channels,
            data_offset: u32::try_from((raw >> 7) & 0x07ff_ffff).map_err(|_| Error::OutputSize)?
                * 32,
            samples: (raw >> 34) as u32,
            extra_chunks: sample_bytes
                .get(chunks_start..samples.pos)
                .ok_or(Error::Invalid("extra chunks"))?,
            data: &[],
            wav_bytes: 0,
        })?;
    }
    if subs.as_slice().first().is_some_and(|s| s.samples == 0) {
        return Err(Error::EmptyFirstSubsound);
    }
    let width = wav::width(header.mode)?;
    let mut total = 0u64;
    for index in 0..count {
        let end = subs
            .as_slice()
            .get(index + 1)
            .map_or(header.data_size, |s| s.data_offset);
        let sub = subs
            .as_mut_slice()
            .get_mut(index)
            .ok_or(Error::Invalid("subsound index"))?;
        let available = end
            .checked_sub(sub.data_offset)
            .ok_or(Error::Invalid("unordered data offsets"))?;
        let section = span(
            data,
            sub.data_offset as usize,
            available as usize,
            "subsound data",
        )?;
        let size = u64::from(sub.samples) * u64::from(sub.channels) * u64::from(width);
        let size = usize::try_from(size).map_err(|_| Error::OutputSize)?;
        sub.data = span(section, 0, size, "PCM samples")?;
        sub.wav_bytes = wav::size(sub, header.mode, convert)?;
        total = total
            .checked_add(sub.wav_bytes as u64)
            .ok_or(Error::OutputSize)?;
    }
    Ok((header, subs, total))
}

#[cfg(test)]
mod tests {
    use super::{Error, wav_frequency};

    #[test]
    fn frequency_uses_float32_ties_to_even_before_integer_validation() {
        for (input, expected) in [
            (1, 1),
            (44100, 44100),
            (16_777_215, 16_777_215),
            (16_777_216, 16_777_216),
            (16_777_217, 16_777_216),
            (16_777_219, 16_777_220),
            (2_147_483_583, 2_147_483_520),
        ] {
            assert_eq!(wav_frequency(input).unwrap(), expected, "{input}");
        }
        for input in [0, 2_147_483_584, i32::MAX as u32, u32::MAX] {
            assert!(matches!(
                wav_frequency(input),
                Err(Error::Invalid("frequency"))
            ));
        }
    }
}
