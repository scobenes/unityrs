//! Bounded metadata walks; image pixels are never decoded or allocated.
// Behavior mirrored from KhronosGroup/glTF-Validator, tag 2.0.0-dev.3.10:
// lib/src/data_access/image_decoder.dart:121-140 (recognition), 166-171 (EOS),
// 207-329 (JPEG), 363-570 (PNG). In particular, PNG returns at the first IDAT
// header (446-448), before its body/CRC or IEND; JPEG returns after SOF/DHP
// (294-296). Both ignore subsequent bytes. CRC bytes are skipped (514-519),
// never checked. IHDR need not be the first chunk; its fields (534-555) are
// metadata, not additional validity constraints. No stricter trailer checks.
use crate::{GltfError, Image, Mime, validate::invalid};
use unityrs_core::limits::Meter;
type Result<T> = std::result::Result<T, GltfError>;
fn bad() -> GltfError {
    invalid("incomplete or invalid image structure for MIME type")
}
fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    let (part, rest) = bytes.split_at_checked(length).ok_or_else(bad)?;
    *bytes = rest;
    Ok(part)
}
fn word(bytes: &[u8]) -> Result<u32> {
    Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| bad())?))
}
pub(crate) fn validate(image: &Image, meter: &Meter) -> Result<()> {
    let mut bytes = image.bytes.as_slice();
    // Every iteration consumes input or returns. Charge even skipped bodies;
    // no length supplied by the image controls an allocation.
    meter.count_decode(bytes.len() as u64)?;
    if bytes.len() < 14 {
        return Err(bad());
    }
    match image.mime {
        Mime::Png if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
            take(&mut bytes, 8)?;
            png(bytes)
        }
        Mime::Jpeg if bytes.starts_with(b"\xff\xd8\xff") => jpeg(bytes),
        _ => Err(bad()),
    }
}
fn jpeg(mut bytes: &[u8]) -> Result<()> {
    take(&mut bytes, 1)?; // Initial FF; SOI is a standalone marker below.
    loop {
        let marker = *take(&mut bytes, 1)?.first().ok_or_else(bad)?;
        // image_decoder.dart:244-248: standalone markers keep the marker state.
        if matches!(marker, 0x01 | 0xd0..=0xd9 | 0xff) {
            continue;
        }
        let length = u16::from_be_bytes(take(&mut bytes, 2)?.try_into().map_err(|_| bad())?);
        let length = usize::from(length.checked_sub(2).ok_or_else(bad)?);
        let segment = take(&mut bytes, length)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf | 0xde) {
            // The validator reads bits, height, width, channels, but never checks
            // component descriptors or requires length == 8 + 3 * channels.
            let header = segment.first_chunk::<6>().ok_or_else(bad)?;
            if !matches!(header.get(5), Some(1 | 3)) {
                return Err(bad());
            }
            return Ok(());
        }
    }
}
fn png(mut bytes: &[u8]) -> Result<()> {
    let mut has_header = false;
    // Also mirror the metadata warnings used by resources_loader.dart:243-246.
    // None = unknown, false = supported, true = custom color information.
    let mut primaries = None;
    let mut transfer = None;
    let mut nonsquare = false;
    loop {
        let length = usize::try_from(word(take(&mut bytes, 4)?)?).map_err(|_| bad())?;
        let kind = take(&mut bytes, 4)?;
        match kind {
            b"IHDR" if length != 13 => return Err(bad()),
            b"cHRM" if length != 32 => return Err(bad()),
            b"sRGB" if length != 1 => return Err(bad()),
            b"pHYs" if length != 9 => return Err(bad()),
            b"gAMA" if length != 4 => return Err(bad()),
            b"IDAT" => {
                return if has_header
                    && primaries != Some(true)
                    && transfer != Some(true)
                    && !nonsquare
                {
                    Ok(())
                } else {
                    Err(bad())
                };
            }
            _ => {}
        }
        let data = take(&mut bytes, length)?;
        match kind {
            b"IHDR" => has_header = true,
            b"iCCP" => {
                primaries = Some(true);
                transfer = Some(true);
            }
            b"sRGB" => {
                primaries = Some(false);
                transfer = Some(false);
            }
            b"gAMA" if transfer.is_none() => {
                transfer = Some(!matches!(word(data)?, 45_455 | 100_000));
            }
            b"cHRM" if primaries.is_none() => {
                // image_decoder.dart:606-639: only these sRGB primaries pass.
                let srgb = [
                    31_270u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000,
                ];
                let mut custom = false;
                for (value, expected) in data.as_chunks::<4>().0.iter().zip(srgb) {
                    custom |= word(value)? != expected;
                }
                primaries = Some(custom);
            }
            b"pHYs" => {
                nonsquare |=
                    word(data.get(..4).ok_or_else(bad)?)? != word(data.get(4..8).ok_or_else(bad)?)?;
            }
            _ => {}
        }
        take(&mut bytes, 4)?; // CRC framing only, exactly as the validator.
    }
}
