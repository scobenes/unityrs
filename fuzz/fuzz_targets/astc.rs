//! `unityrs-texture`'s ASTC LDR decoder on a texture's size and blocks, as
//! `unityrs_fuzz::astc` describes. The input is
//! `unityrs_testkit::fuzz::TextureCase`: the fields (its format one of the
//! twelve), the meter, the platform blob, then the data.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::astc::check(data));
