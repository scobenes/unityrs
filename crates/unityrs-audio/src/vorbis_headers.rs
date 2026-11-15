//! Caller-owned setup packets. No FMOD-derived table is embedded or distributed.
//!
//! The VST1 interchange format is little endian: magic `VST1`, u32 entry
//! count, then strictly increasing (u32 CRC, u32 byte length, setup packet)
//! records. Packets include the `\x05vorbis` prefix. No trailing bytes.
//! Convert an externally obtained table outside the repository; do not save
//! that table or anything derived from it as a fixture.
use crate::{Error, send_sync};
use std::{io::Read, path::Path, sync::Arc};
use unityrs_core::limits::{Meter, MeteredVec};

/// Runtime setup headers, retaining their memory charge. The caller supplies
/// a path; there is no default path, network fetch or embedded fallback.
#[derive(Debug)]
pub struct VorbisHeaders {
    bytes: MeteredVec<u8>,
}
const _: () = send_sync::<VorbisHeaders>();
impl VorbisHeaders {
    /// Bound the provider even when the path names a growing file or stream.
    pub const MAX_BYTES: usize = 64 << 20;

    pub fn load(path: &Path, meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut file = std::fs::File::open(path).map_err(Error::VorbisHeadersIo)?;
        let mut bytes = MeteredVec::new(meter);
        let mut block = [0u8; 8192];
        loop {
            let n = file.read(&mut block).map_err(Error::VorbisHeadersIo)?;
            if n == 0 {
                break;
            }
            if bytes.len().saturating_add(n) > Self::MAX_BYTES {
                return Err(Error::Invalid("Vorbis table size"));
            }
            meter.count_decode(n as u64)?;
            bytes.extend_from_slice(block.get(..n).ok_or(Error::OutputSize)?)?;
        }
        Self::validate(bytes, meter)
    }

    /// Load an in-memory VST1 table with the same validation as [`Self::load`].
    pub fn from_bytes(input: &[u8], meter: &Arc<Meter>) -> Result<Self, Error> {
        if input.len() > Self::MAX_BYTES {
            return Err(Error::Invalid("Vorbis table size"));
        }
        meter.count_decode(input.len() as u64)?;
        // Validate before allocating an input-sized copy.
        validate(input)?;
        let mut bytes = MeteredVec::with_capacity(meter, input.len())?;
        bytes.extend_from_slice(input)?;
        Ok(Self { bytes })
    }
    fn validate(bytes: MeteredVec<u8>, meter: &Arc<Meter>) -> Result<Self, Error> {
        meter.count_decode(bytes.len() as u64)?;
        validate(bytes.as_slice())?;
        Ok(Self { bytes })
    }
    #[cfg_attr(
        not(feature = "vorbis"),
        allow(dead_code, reason = "used by the optional Vorbis decoder")
    )]
    pub(crate) fn get(&self, crc: u32, meter: &Arc<Meter>) -> Result<&[u8], Error> {
        let mut rest = self
            .bytes
            .as_slice()
            .get(8..)
            .ok_or(Error::Invalid("Vorbis table"))?;
        while !rest.is_empty() {
            meter.count_decode(8)?;
            let key = word(&mut rest)?;
            let size = word(&mut rest)? as usize;
            let packet = take(&mut rest, size)?;
            if key == crc {
                return Ok(packet);
            }
            if key > crc {
                break;
            }
        }
        Err(Error::VorbisSetupMissing { crc })
    }
}
fn validate(mut bytes: &[u8]) -> Result<(), Error> {
    if take(&mut bytes, 4)? != b"VST1" {
        return Err(Error::Invalid("Vorbis table magic"));
    }
    let count = word(&mut bytes)?;
    if count > 65536 || count as usize > bytes.len() / 15 {
        return Err(Error::Invalid("Vorbis table count"));
    }
    let mut previous = None;
    for _ in 0..count {
        let key = word(&mut bytes)?;
        if previous.is_some_and(|p| key <= p) {
            return Err(Error::Invalid("Vorbis table key order"));
        }
        previous = Some(key);
        let size = word(&mut bytes)? as usize;
        if size > 65535 || !take(&mut bytes, size)?.starts_with(b"\x05vorbis") {
            return Err(Error::Invalid("Vorbis setup packet"));
        }
    }
    if !bytes.is_empty() {
        return Err(Error::Invalid("Vorbis table trailing bytes"));
    }
    Ok(())
}
fn word(bytes: &mut &[u8]) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        take(bytes, 4)?
            .try_into()
            .map_err(|_| Error::Invalid("Vorbis table word"))?,
    ))
}
fn take<'a>(bytes: &mut &'a [u8], size: usize) -> Result<&'a [u8], Error> {
    let (head, tail) = bytes
        .split_at_checked(size)
        .ok_or(Error::Invalid("Vorbis table bounds"))?;
    *bytes = tail;
    Ok(head)
}
