//! `check_file_type` (`ImportHelper.check_file_type`) over any bytes, under
//! the fuzz limits (`unityrs_fuzz::limits`), whose `max_inflate` bounds the
//! gzip check's inflation.
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::detect::check_file_type;

fuzz_target!(|data: &[u8]| {
    let _ = check_file_type(data, &unityrs_fuzz::limits());
});
