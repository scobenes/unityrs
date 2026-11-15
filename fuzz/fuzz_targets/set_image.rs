//! Raw set_image conversion, mip, platform and memory gates.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    unityrs_fuzz::set_image::check(data);
});
