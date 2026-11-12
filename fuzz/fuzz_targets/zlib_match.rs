//! The derived deflate versus pristine zlib 1.2.13 at Pillow's settings.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = &data[..data.len().min(4 << 20)];
    unityrs_imageio::test_reference::assert_deflate_equal(input, 257, 509);
});
