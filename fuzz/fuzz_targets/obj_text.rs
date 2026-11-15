#![no_main]
libfuzzer_sys::fuzz_target!(|input: &[u8]| {
    unityrs_fuzz::obj_text::check(input);
});
