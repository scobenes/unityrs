//! Legacy CRN headers, Huffman tables, palettes and mip-zero streams through
//! `unityrs-texture`'s decoder, as `unityrs_fuzz::crunch` describes.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::crunch::check(data));
