//! Load then save then load, preserving objects and producing idempotent saves.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    unityrs_fuzz::with_stack(|| unityrs_fuzz::serialized_save::check(data));
});
