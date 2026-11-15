//! Default image writers under bounded pixels and allocation budgets.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::image_writers::check(data));
