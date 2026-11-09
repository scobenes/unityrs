//! Plan section 8's R9: unityrs's LZ4 block decoder against liblz4 1.9.4's
//! `LZ4_decompress_safe`, as `unityrs_fuzz::lz4` describes. The input is
//! `unityrs_testkit::fuzz::lz4_case`: the declared size, then the block.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::lz4::check(data));
