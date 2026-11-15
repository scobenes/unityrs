//! Bounded FSB5 parsing and PCM/passthrough output; raw input, no oracle dependency.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| unityrs_fuzz::fsb5::check(data));
