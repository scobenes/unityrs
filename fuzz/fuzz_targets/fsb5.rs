//! Bounded FSB5 parsing and PCM/IMA/FADPCM/passthrough output; no oracle dependency.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| unityrs_fuzz::fsb5::check(data));
