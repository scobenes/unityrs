// Ported from UnityPy 1.25.2 (MIT): export/AudioClipConverter.py and enums/Audio.py. See THIRD_PARTY_LICENSES.
// Ported from fmod_toolkit 0.1.3 (MIT): fmod.py subsound filenames. See THIRD_PARTY_LICENSES.
//! `AudioClip` dispatch and FSB5 PCM/ADPCM/Vorbis exports, with no FMOD dependency.
//!
//! [`plan`] validates the real bank boundaries and bounds WAV output before
//! decoding. Its metadata, every output name and payload are metered and
//! allocated fallibly. Vorbis uses an explicit runtime setup provider; lewton
//! alone allocates infallibly, so audio stays off batch. Plans borrow the input,
//! preventing decode against a
//! different bank. Work is charged before traversal or output writes.
//!
//! UnityPy's magic dispatch is export/AudioClipConverter.py:35-49. Contrary
//! to older converters, this release never dispatches on `m_Type`; legacy
//! non-passthrough data is [`Error::UnsupportedContainer`] (Tier U).
//! [`extension`] reproduces the shared-enum table and property, independently
//! of sample dispatch (classes/legacy_patch/AudioClip.py:5-6).
//!
//! Integer PCM is copied, float defaults to numpy's saturated int16 rule.
//! WAVs shorter than four data bytes are Tier C (`fmod_toolkit` raises).
//! A bank with an empty first subsound is Tier U and refused. Other codecs
//! are typed unsupported errors, deferred to optional future work.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

mod adpcm;
mod fsb5;
mod vorbis;
mod vorbis_headers;
mod wav;

pub use fsb5::{Header, Subsound};
use std::sync::Arc;
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
pub use vorbis_headers::VorbisHeaders;

/// A malformed bank field, a deliberate format refusal, or resource exhaustion.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated {field} at {offset}: need {length} bytes")]
    Truncated {
        field: &'static str,
        offset: usize,
        length: usize,
    },
    #[error("invalid FSB5 field: {0}")]
    Invalid(&'static str),
    #[error("unsupported FSB5 version {0}")]
    Version(u32),
    #[error("unsupported FSB5 codec {codec}")]
    Unsupported { codec: u32 },
    #[error("unsupported audio container (including legacy non-passthrough audio)")]
    UnsupportedContainer,
    #[error("unsupported FSB5 bank with no samples in its first subsound (Tier U)")]
    EmptyFirstSubsound,
    #[error("FSB5 channel count {channels} exceeds FMOD's maximum of 32")]
    TooManyChannels { channels: u16 },
    #[error("Vorbis requires setup headers loaded from a caller-supplied path")]
    VorbisHeadersRequired,
    #[error("cannot read the caller-supplied Vorbis setup table: {0}")]
    VorbisHeadersIo(std::io::Error),
    #[error("Vorbis setup header is absent for CRC {crc:#010x}")]
    VorbisSetupMissing { crc: u32 },
    #[error("unsupported Vorbis subsound with fewer than four WAV data bytes (Tier U)")]
    VorbisShortSubsound,
    #[error("Vorbis decoder panicked on malformed data")]
    VorbisPanicked,
    #[error("audio output exceeds WAV's signed 32-bit fields or address space")]
    OutputSize,
    #[error(transparent)]
    Charge(#[from] ChargeError),
}
const _: () = send_sync::<Error>();

/// A validated, borrowed decode recipe. Output bytes exclude filenames and
/// metadata; their allocations are also charged by `decode` before creation.
#[derive(Debug)]
pub struct Plan<'a> {
    kind: Kind<'a>,
    output_bytes: u64,
    convert_float: bool,
}
const _: () = send_sync::<Plan<'static>>();
#[derive(Debug)]
enum Kind<'a> {
    Pass {
        data: &'a [u8],
        extension: &'static [u8],
    },
    Bank {
        header: Header,
        subsounds: MeteredVec<Subsound<'a>>,
    },
}
impl<'a> Plan<'a> {
    #[must_use]
    pub const fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
    #[must_use]
    pub const fn header(&self) -> Option<&Header> {
        match &self.kind {
            Kind::Bank { header, .. } => Some(header),
            Kind::Pass { .. } => None,
        }
    }
    #[must_use]
    pub fn subsounds(&self) -> &[Subsound<'a>] {
        match &self.kind {
            Kind::Bank { subsounds, .. } => subsounds.as_slice(),
            Kind::Pass { .. } => &[],
        }
    }
}

/// One sample export, retaining the charges for its raw UTF-8/surrogateescape
/// filename bytes and file content. Callers apply their own path policy.
#[derive(Debug)]
pub struct Sample {
    pub name: MeteredVec<u8>,
    pub bytes: MeteredVec<u8>,
}
const _: () = send_sync::<Sample>();

/// The final 20 keys of UnityPy's `AUDIO_TYPE_EXTEMSION` table. Values 1 and
/// 2 are overwritten by compression formats (enums/Audio.py:33-56).
#[must_use]
pub const fn extension(format: Option<i32>) -> &'static str {
    match format {
        Some(0..=3 | 8 | 24) => ".fsb",
        Some(4 | 5 | 23) => ".vag",
        Some(6 | 20 | 22) => ".wav",
        Some(7) => ".m4a",
        Some(9) => ".at9",
        Some(10) => ".it",
        Some(12) => ".mod",
        Some(13) => ".mp3",
        Some(14) => ".ogg",
        Some(17) => ".s3m",
        Some(21) => ".xm",
        _ => ".audioclip",
    }
}

/// Inspect magic, then validate an FSB5 bank and compute its total WAV size.
/// This reads metadata only, never allocates or decodes a PCM payload.
pub fn plan<'a>(
    data: &'a [u8],
    convert_float: bool,
    meter: &Arc<Meter>,
) -> Result<Plan<'a>, Error> {
    meter.count_decode(8.min(data.len()) as u64)?;
    let suffix: Option<&'static [u8]> = if data.starts_with(b"OggS") {
        Some(b".ogg")
    } else if data.starts_with(b"RIFF") {
        Some(b".wav")
    } else if data.get(4..8) == Some(b"ftyp") {
        Some(b".m4a")
    } else {
        None
    };
    if let Some(extension) = suffix {
        return Ok(Plan {
            kind: Kind::Pass { data, extension },
            output_bytes: data.len() as u64,
            convert_float,
        });
    }
    if !data.starts_with(b"FSB5") {
        return Err(Error::UnsupportedContainer);
    }
    let (header, subsounds, output_bytes) = fsb5::parse(data, convert_float, meter)?;
    Ok(Plan {
        kind: Kind::Bank { header, subsounds },
        output_bytes,
        convert_float,
    })
}

/// Materialize the plan's files. Names follow `fmod_toolkit` 0.1.3
/// `fmod.py` lines 77-98: the clip name, then a numeric suffix; the bank's
/// own name table does not determine export filenames.
pub fn decode(
    plan: &Plan<'_>,
    name: &[u8],
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Sample>, Error> {
    decode_with_vorbis(plan, name, None, meter)
}

/// Decode with an explicit runtime Vorbis setup provider. WAV output is
/// charged before lewton is entered. Lewton's internal allocations are
/// infallible and unmetered (P3 decision 19); never use this on a batch path.
pub fn decode_with_vorbis(
    plan: &Plan<'_>,
    name: &[u8],
    headers: Option<&VorbisHeaders>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Sample>, Error> {
    if plan.header().is_some_and(|h| h.mode == 15) && headers.is_none() {
        return Err(Error::VorbisHeadersRequired);
    }
    let count = match &plan.kind {
        Kind::Pass { .. } => 1,
        Kind::Bank { subsounds, .. } => subsounds.len(),
    };
    meter.count_decode(count as u64)?;
    let mut files = MeteredVec::with_capacity(meter, count)?;
    match &plan.kind {
        Kind::Pass { data, extension } => {
            let filename = filename(name, 0, extension, meter)?;
            meter.count_decode(data.len() as u64)?;
            let mut bytes = MeteredVec::with_capacity(meter, data.len())?;
            bytes.extend_from_slice(data)?;
            files.push(Sample {
                name: filename,
                bytes,
            })?;
        }
        Kind::Bank { header, subsounds } => {
            for (i, sub) in subsounds.as_slice().iter().enumerate() {
                let filename = filename(name, i, b".wav", meter)?;
                let bytes = wav::decode(sub, header.mode, plan.convert_float, headers, meter)?;
                files.push(Sample {
                    name: filename,
                    bytes,
                })?;
            }
        }
    }
    Ok(files)
}
fn filename(
    name: &[u8],
    index: usize,
    extension: &[u8],
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    let mut digits = [0u8; 20];
    let mut pos = digits.len();
    let mut n = index;
    while n > 0 {
        pos = pos.checked_sub(1).ok_or(Error::OutputSize)?;
        *digits.get_mut(pos).ok_or(Error::OutputSize)? =
            b'0' + u8::try_from(n % 10).map_err(|_| Error::OutputSize)?;
        n /= 10;
    }
    let suffix = digits.get(pos..).ok_or(Error::OutputSize)?;
    let size = name
        .len()
        .checked_add(extension.len())
        .and_then(|n| n.checked_add(suffix.len() + usize::from(index > 0)))
        .ok_or(Error::OutputSize)?;
    meter.count_decode(size as u64)?;
    let mut out = MeteredVec::with_capacity(meter, size)?;
    out.extend_from_slice(name)?;
    if index > 0 {
        out.push(b'-')?;
        out.extend_from_slice(suffix)?;
    }
    out.extend_from_slice(extension)?;
    Ok(out)
}
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

#[cfg(test)]
mod tests {
    #[test]
    fn public_types_are_send_sync_and_decoders_never_wait() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(
            missing.is_empty(),
            "public types missing Send + Sync: {missing:?}"
        );
        let waits = unityrs_testkit::waits::waits(&src);
        assert!(
            waits.is_empty(),
            "decoder starts threads or waits: {waits:?}"
        );
    }
}
