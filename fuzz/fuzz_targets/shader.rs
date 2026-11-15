//! Bounded shader program parsing, repeated on fresh meters to check a
//! deterministic verdict and complete release of all temporary charges.
//! The same input also describes an arbitrary typetree value for conversion.
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_core::walk::{ReadCtx, boost};
use unityrs_testkit::fuzz::ShaderCase;

fuzz_target!(|data: &[u8]| {
    if data.len() as u64 > unityrs_fuzz::WORK {
        return;
    }
    let case = ShaderCase::decode(data);
    let memory = if case.meter == 0 {
        unityrs_fuzz::MEMORY
    } else {
        u64::from(case.meter).pow(2) * 64
    };
    let run = || {
        let meter = Meter::with_decode_work(
            memory,
            unityrs_fuzz::WORK,
            std::sync::Arc::new(Unbounded),
            Reservation::Input,
        );
        let (result, attempted) =
            unityrs_fuzz::shader::render_all(&case.blob, case.version, &meter);
        let answer = (result.map_err(|e| format!("{e:?}")), attempted);
        assert_eq!(meter.used(), 0, "shader parser retained a memory charge");
        answer
    };
    assert_eq!(run(), run(), "shader parser changed its answer");
    let Some(value) = unityrs_fuzz::Case::new(data) else {
        return;
    };
    unityrs_fuzz::with_stack(|| {
        let read = ReadCtx {
            endian: value.endian(),
            ref_types: value.ref_types(),
            options: &value.options,
            meter: &value.meter,
        };
        if let Ok(parsed) = boost::read(&value.data, &value.tree, &read) {
            let meter = value.new_meter();
            let result = unityrs_export::shader::export_parsed(&parsed, (2021, 3), &meter);
            drop(result);
            assert_eq!(meter.used(), 0, "shader converter retained a memory charge");
        }
    });
});
