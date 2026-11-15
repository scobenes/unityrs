// Ported from python-fsb5 (MIT): fsb5/vorbis.py rebuild and rebuild_id_header. See THIRD_PARTY_LICENSES.
// Ported from Fmod5Sharp (MIT): CodecRebuilders/FmodVorbisRebuilder.cs packet framing and sample-count trimming. See THIRD_PARTY_LICENSES.
//! Rebuild the identification and setup headers, then decode length-prefixed
//! FSB5 packets directly. Ogg page construction and mode-bit/granule parsing
//! are unnecessary with lewton's packet API: its overlap state computes the
//! same sample progression. `Fmod5Sharp` bounds the final output at the bank's
//! sample count. python-fsb5 supplies the fixed 256/2048 block sizes.
//!
//! lewton 0.10.2 (MIT OR Apache-2.0), without its optional Ogg dependency,
//! supplies the pure Rust packet decoder. Internal allocations are infallible
//! and outside the meter; P3 decision 19 explicitly isolates audio from batch.
use crate::{Error, Subsound, VorbisHeaders};
#[cfg(feature = "vorbis")]
use lewton::{audio::PreviousWindowRight, header};
#[cfg(feature = "vorbis")]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use unityrs_core::limits::Meter;

pub(crate) fn validate(sub: &Subsound<'_>, meter: &Arc<Meter>) -> Result<(), Error> {
    if sub.wav_bytes < 48 {
        return Err(Error::VorbisShortSubsound);
    }
    crc(sub)?;
    meter.count_decode(sub.data.len() as u64)?;
    let mut data = sub.data;
    let mut count = 0;
    while packet(&mut data)?.is_some() {
        count += 1;
    }
    if count == 0 {
        return Err(Error::Invalid("Vorbis packets absent"));
    }
    Ok(())
}
fn crc(sub: &Subsound<'_>) -> Result<u32, Error> {
    let mut chunks = sub.extra_chunks;
    let mut found = None;
    while !chunks.is_empty() {
        let raw = u32::from_le_bytes(
            take(&mut chunks, 4)?
                .try_into()
                .map_err(|_| Error::Invalid("Vorbis chunk"))?,
        );
        let payload = take(&mut chunks, ((raw >> 1) & 0x00ff_ffff) as usize)?;
        if raw >> 25 == 11 {
            if found.is_some() {
                return Err(Error::Invalid("duplicate VORBISDATA"));
            }
            found = Some(u32::from_le_bytes(
                payload
                    .get(..4)
                    .ok_or(Error::Invalid("VORBISDATA size"))?
                    .try_into()
                    .map_err(|_| Error::Invalid("VORBISDATA CRC"))?,
            ));
        }
    }
    found.ok_or(Error::Invalid("VORBISDATA absent"))
}
fn take<'a>(data: &mut &'a [u8], size: usize) -> Result<&'a [u8], Error> {
    let (head, tail) = data
        .split_at_checked(size)
        .ok_or(Error::Invalid("Vorbis packet bounds"))?;
    *data = tail;
    Ok(head)
}
fn packet<'a>(data: &mut &'a [u8]) -> Result<Option<&'a [u8]>, Error> {
    if data.is_empty() {
        return Ok(None);
    }
    let size = u16::from_le_bytes(
        take(data, 2)?
            .try_into()
            .map_err(|_| Error::Invalid("Vorbis packet length"))?,
    );
    if size == 0 || size == u16::MAX {
        // Both sentinels occur in FSB5; remaining bytes are bank alignment.
        *data = &[];
        return Ok(None);
    }
    Ok(Some(take(data, usize::from(size))?))
}
#[cfg(feature = "vorbis")]
fn ident(sub: &Subsound<'_>) -> Result<[u8; 30], Error> {
    let mut out = [0u8; 30];
    out.get_mut(..7)
        .ok_or(Error::OutputSize)?
        .copy_from_slice(b"\x01vorbis");
    *out.get_mut(11).ok_or(Error::OutputSize)? =
        u8::try_from(sub.channels).map_err(|_| Error::Invalid("Vorbis channels"))?;
    out.get_mut(12..16)
        .ok_or(Error::OutputSize)?
        .copy_from_slice(&sub.frequency.to_le_bytes());
    *out.get_mut(28).ok_or(Error::OutputSize)? = 0xb8;
    *out.get_mut(29).ok_or(Error::OutputSize)? = 1;
    Ok(out)
}

/// The caller has already allocated and charged the entire WAV. Contain
/// panics from lewton on malformed setup/audio packets; no panic escapes.
/// Allocator aborts cannot be caught, which is why this is never a batch API.
#[cfg(feature = "vorbis")]
pub(crate) fn decode(
    sub: &Subsound<'_>,
    headers: &VorbisHeaders,
    out: &mut [u8],
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    let setup = headers.get(crc(sub)?, meter)?;
    meter.count_decode(setup.len() as u64)?;
    contain(|| {
        let id = header::read_header_ident(&ident(sub)?)
            .map_err(|_| Error::Invalid("Vorbis identification"))?;
        let setup =
            header::read_header_setup(setup, id.audio_channels, (id.blocksize_0, id.blocksize_1))
                .map_err(|_| Error::Invalid("Vorbis setup"))?;
        let mut previous = PreviousWindowRight::new();
        let mut data = sub.data;
        let channels = usize::from(sub.channels);
        // Output-to-stream indices observed with FMOD 2.00.22. Other
        // counts (including 3/5/7) retain Vorbis order; see PARITY 16.
        let channel_order: &[usize] = match channels {
            6 => &[0, 2, 1, 5, 3, 4],
            8 => &[0, 2, 1, 7, 5, 6, 3, 4],
            _ => &[],
        };
        let mut written = 0;
        while let Some(packet) = packet(&mut data)? {
            // Charge a maximum block's decode work before entering lewton.
            meter.count_decode((2048 * channels + packet.len()) as u64)?;
            let pcm = lewton::audio::read_audio_packet(&id, &setup, packet, &mut previous)
                .map_err(|_| Error::Invalid("Vorbis audio packet"))?;
            let frames = pcm
                .first()
                .ok_or(Error::Invalid("Vorbis decoded channels"))?
                .len();
            for frame in 0..frames {
                if written == out.len() {
                    return Ok(());
                }
                for channel in 0..channels {
                    let source = channel_order.get(channel).copied().unwrap_or(channel);
                    let sample = pcm
                        .get(source)
                        .and_then(|c| c.get(frame))
                        .ok_or(Error::Invalid("Vorbis decoded frame"))?;
                    out.get_mut(written..written + 2)
                        .ok_or(Error::OutputSize)?
                        .copy_from_slice(&sample.to_le_bytes());
                    written += 2;
                }
            }
            if written == out.len() {
                return Ok(());
            }
        }
        Err(Error::Invalid("Vorbis sample count exceeds packets"))
    })
}
#[cfg(feature = "vorbis")]
fn contain<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| Error::VorbisPanicked)?
}

#[cfg(not(feature = "vorbis"))]
pub(crate) fn decode(
    _: &Subsound<'_>,
    _: &VorbisHeaders,
    _: &mut [u8],
    _: &Arc<Meter>,
) -> Result<(), Error> {
    Err(Error::Unsupported { codec: 15 })
}
