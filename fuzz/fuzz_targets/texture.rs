//! `unityrs-texture`'s `select` and `decode` on a texture's fields and data,
//! as `unityrs_fuzz::texture` describes. The input is
//! `unityrs_testkit::fuzz::TextureCase`: the fields, the meter, the platform
//! blob, then the data.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::texture::check(data));
