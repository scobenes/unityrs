//! UnityPyBoost's reader (`walk::boost::read`) over a fuzzed tree, ref
//! types and bytes (`unityrs_testkit::fuzz::TreeCase`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::walk::{ReadCtx, boost};

fuzz_target!(|data: &[u8]| {
    let ctx = unityrs_fuzz::context();
    let meter = unityrs_fuzz::meter(&ctx);
    let Some(case) = unityrs_fuzz::Case::new(data, &meter) else {
        return;
    };
    unityrs_fuzz::with_stack(|| {
        let read = ReadCtx {
            endian: case.endian(),
            ref_types: case.ref_types(),
            options: &case.options,
            meter: &meter,
        };
        let _ = boost::read(&case.data, &case.tree, &read);
    });
});
