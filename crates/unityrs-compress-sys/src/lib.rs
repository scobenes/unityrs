//! Safe compression bridges. The only unsafe code is inside this crate:
//! callers supply LZ4's state/output slices and meter LZMA's reported memory
//! before entering its fallible allocator bridge.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

mod lzma;
pub use lzma::{LzmaError, lzma_compress, lzma_encoder_memory};

/// A native encoder refusal, with no allocated diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input or output length cannot be passed to the native ABI.
    Range,
    /// Rust's global allocator refused this many bytes.
    Allocator(u64),
    /// A native allocation exceeded its caller's prepaid allowance.
    Allowance,
    /// The encoder could not complete its stream.
    Compress,
}

unsafe extern "C" {
    safe fn unityrs_LZ4_versionNumber() -> c_int;
    safe fn unityrs_LZ4_sizeofStateHC() -> c_int;
    safe fn unityrs_LZ4_compressBound(input: c_int) -> c_int;
    fn unityrs_LZ4_compress_HC_extStateHC(
        state: *mut c_void,
        src: *const c_char,
        dst: *mut c_char,
        src_size: c_int,
        dst_capacity: c_int,
        level: c_int,
    ) -> c_int;
}

/// Version number of the pristine library linked here: 10904 for 1.9.4.
#[must_use]
pub fn lz4_version_number() -> u32 {
    u32::try_from(unityrs_LZ4_versionNumber()).unwrap_or(0)
}

/// Eight-byte words needed for this build's fixed-size, aligned HC state.
pub fn lz4_state_words() -> Result<usize, Error> {
    let bytes = usize::try_from(unityrs_LZ4_sizeofStateHC()).map_err(|_| Error::Range)?;
    Ok(bytes.div_ceil(size_of::<u64>()))
}

/// Maximum compressed size of an input accepted by liblz4 1.9.4.
pub fn lz4_bound(input: usize) -> Result<usize, Error> {
    let input = c_int::try_from(input).map_err(|_| Error::Range)?;
    let bound = unityrs_LZ4_compressBound(input);
    if bound <= 0 {
        return Err(Error::Range);
    }
    usize::try_from(bound).map_err(|_| Error::Range)
}

/// Compress with pristine liblz4 1.9.4 HC level 9 into caller-owned storage.
pub fn lz4_compress(data: &[u8], output: &mut [u8], state: &mut [u64]) -> Result<usize, Error> {
    if state.len() < lz4_state_words()? || output.len() < lz4_bound(data.len())? {
        return Err(Error::Range);
    }
    let input = c_int::try_from(data.len()).map_err(|_| Error::Range)?;
    let bound = c_int::try_from(output.len()).map_err(|_| Error::Range)?;
    // SAFETY: state has at least the size/alignment reported by this exact
    // compiled library. Live nonoverlapping input/output lengths fit c_int.
    // HC level 9 is supported, and this entry point performs no allocation.
    let written = unsafe {
        unityrs_LZ4_compress_HC_extStateHC(
            state.as_mut_ptr().cast(),
            data.as_ptr().cast(),
            output.as_mut_ptr().cast(),
            input,
            bound,
            9,
        )
    };
    if written <= 0 {
        return Err(Error::Compress);
    }
    usize::try_from(written).map_err(|_| Error::Range)
}
