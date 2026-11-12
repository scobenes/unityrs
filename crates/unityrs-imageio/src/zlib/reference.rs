//! Test/fuzz-only comparison with pristine zlib 1.2.13. This module is
//! compiled only with `test-reference`, never needed by the PNG writer.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::{
    Deflater, LEVEL, MEM_LEVEL, STRATEGY, WINDOW_BITS, Z_DEFLATED, Z_FINISH, Z_NO_FLUSH, Z_OK,
    Z_STREAM_END, ZStream, c_char, c_int, c_uint, z_deflate, z_deflateEnd, z_deflateInit2_,
};

type Init = unsafe extern "C" fn(
    *mut ZStream,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    *const c_char,
    c_int,
) -> c_int;
type Run = unsafe extern "C" fn(*mut ZStream, c_int) -> c_int;
type End = unsafe extern "C" fn(*mut ZStream) -> c_int;

unsafe extern "C" {
    fn unityrs_zref_deflateInit2_(
        stream: *mut ZStream,
        level: c_int,
        method: c_int,
        window_bits: c_int,
        mem_level: c_int,
        strategy: c_int,
        version: *const c_char,
        stream_size: c_int,
    ) -> c_int;
    fn unityrs_zref_deflate(stream: *mut ZStream, flush: c_int) -> c_int;
    fn unityrs_zref_deflateEnd(stream: *mut ZStream) -> c_int;
}

struct Stream<'a> {
    inner: &'a mut ZStream,
    run: Run,
    end: End,
}

impl Stream<'_> {
    fn pump(&mut self, output: &mut [u8], flush: c_int, bytes: &mut Vec<u8>) -> c_int {
        self.inner.next_out = output.as_mut_ptr();
        self.inner.avail_out = c_uint::try_from(output.len()).unwrap();
        // SAFETY: the stream is pinned and live, its current input remains
        // borrowed by compress until consumed, and output lives for this call.
        let code = unsafe { (self.run)(self.inner, flush) };
        assert!(code == Z_OK || code == Z_STREAM_END, "deflate: {code}");
        let used = output.len() - usize::try_from(self.inner.avail_out).unwrap();
        bytes.extend_from_slice(&output[..used]);
        code
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        // SAFETY: the matching init function succeeded, the pinned backing
        // Deflater and its allocator allowance still live, and only this
        // guard ends the stream (Deflater.live remains false).
        unsafe { (self.end)(self.inner) };
    }
}

fn compress(input: &[u8], input_chunk: usize, output_chunk: usize, reference: bool) -> Vec<u8> {
    let (init, run, end): (Init, Run, End) = if reference {
        (
            unityrs_zref_deflateInit2_,
            unityrs_zref_deflate,
            unityrs_zref_deflateEnd,
        )
    } else {
        (z_deflateInit2_, z_deflate, z_deflateEnd)
    };
    // Reuse the production stream layout, initial values and allocators.
    let mut backing = std::pin::pin!(Deflater::new());
    // SAFETY: neither stream nor allowance moves until the guard ends it.
    let backing = unsafe { backing.as_mut().get_unchecked_mut() };
    backing.stream.opaque = (&raw mut backing.allowance).cast();
    // SAFETY: as in Deflater::init; both implementations have the same ABI.
    let code = unsafe {
        init(
            &raw mut backing.stream,
            LEVEL,
            Z_DEFLATED,
            WINDOW_BITS,
            MEM_LEVEL,
            STRATEGY,
            c"1.2.13".as_ptr(),
            c_int::try_from(size_of::<ZStream>()).unwrap(),
        )
    };
    assert_eq!(code, Z_OK, "deflateInit2");
    let mut stream = Stream {
        inner: &mut backing.stream,
        run,
        end,
    };
    let mut bytes = Vec::new();
    let mut output = vec![0; output_chunk];
    for part in input.chunks(input_chunk) {
        stream.inner.next_in = part.as_ptr();
        stream.inner.avail_in = c_uint::try_from(part.len()).unwrap();
        while stream.inner.avail_in != 0 {
            assert_eq!(stream.pump(&mut output, Z_NO_FLUSH, &mut bytes), Z_OK);
        }
    }
    while stream.pump(&mut output, Z_FINISH, &mut bytes) != Z_STREAM_END {}
    bytes
}

/// Assert byte equality with pristine zlib at Pillow's settings, with the
/// same input/output chunk sizes. Test and fuzz support only.
///
/// # Panics
/// Panics on unequal bytes, zlib failure, or a chunk size outside 1..=1 MiB.
pub fn assert_deflate_equal(input: &[u8], input_chunk: usize, output_chunk: usize) {
    assert!((1..=1 << 20).contains(&input_chunk));
    assert!((1..=1 << 20).contains(&output_chunk));
    let actual = compress(input, input_chunk, output_chunk, false);
    let reference = compress(input, input_chunk, output_chunk, true);
    assert_eq!(
        actual.len(),
        reference.len(),
        "compressed length for {} input bytes",
        input.len()
    );
    // Report the first offset, without dumping megabytes on a mismatch.
    assert_eq!(
        actual.iter().zip(&reference).position(|(a, b)| a != b),
        None,
        "compressed bytes differ for {} input bytes",
        input.len()
    );
}
