#![no_main]
libfuzzer_sys::fuzz_target!(|input: &[u8]| {
    unityrs_fuzz::anim_clip::check(input);
});
