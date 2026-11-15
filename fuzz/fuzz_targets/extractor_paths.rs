//! Paths plus serialized extraction into a bounded in-memory sink.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    unityrs_fuzz::with_stack(|| {
        unityrs_fuzz::extractor_paths::check(data);
    });
});
