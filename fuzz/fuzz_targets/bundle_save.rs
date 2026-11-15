//! Bundle load/save/load invariants using the existing bundle seeds.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    unityrs_fuzz::with_stack(|| unityrs_fuzz::bundle_save::check(data));
});
