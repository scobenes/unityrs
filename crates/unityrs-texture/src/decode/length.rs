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
