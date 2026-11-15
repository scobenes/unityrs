//! Input lengths, format footprints and scratch charge estimates.

use super::{Format, Library, astc, crunch};

/// The length Pillow 9.5.0's `BCn` decoder sees for `len` bytes of data:
/// `ImagingBcnDecode` passes its `Py_ssize_t` length to `decode_bcn`'s `int`
/// parameter, a conversion GCC and Clang both make modulo 2^32, then as a
/// signed value (C leaves an out-of-range conversion to the
/// implementation), so the decode loop (`while (bytes >= SZ)`) runs on the
/// length's low 32 bits, none when bit 31 is set. Below 2 GiB it is the
/// length itself.
#[must_use]
pub fn bcn_length(len: usize) -> u64 {
    let low = u64::try_from(len).unwrap_or(u64::MAX) & 0xFFFF_FFFF;
    // As a signed 32-bit value: negative when bit 31 is set, and then the
    // loop decodes no block.
    if low >= 1 << 31 { 0 } else { low }
}

/// The temporary buffers and tables needed beside a decode's output.
pub(super) fn scratch_charge(format: Format, unpacked_bytes: u64, width: u64) -> u64 {
    if format.is_unity_crunched() {
        crunch::unity::scratch(unpacked_bytes, width)
    } else if format.is_crunched() {
        crunch::scratch(unpacked_bytes)
    } else {
        astc_footprint(format).map_or(0, astc::Tables::charge)
    }
}

/// Stored bytes per pixel for raw Pillow and closed-form formats.
pub(super) fn raw_pixel_bytes(format: Format) -> u64 {
    crate::tierb::layout(format).map_or_else(
        || u64::from(format.raw_bits().unwrap_or(0) / 8),
        crate::tierb::Layout::bytes,
    )
}

/// An ASTC format's footprint.
pub(super) fn astc_footprint(format: Format) -> Option<astc::Footprint> {
    if format.library() != Library::Astcenc {
        return None;
    }
    let (w, h) = format.block_size()?;
    astc::Footprint::new(w, h)
}
