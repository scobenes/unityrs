//! The pure-Python reader (`walk::python::read_with_reader`) over a fuzzed
//! tree, ref types and bytes (`unityrs_testkit::fuzz::TreeCase`), from the
//! case's start, by the memory reader's rules (any position, negative ones
//! included) or the stream reader's (a position a `seek` took, never
//! negative).
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::stream::ReaderKind;
use unityrs_core::walk::{ReadCtx, python};
use unityrs_testkit::fuzz::TreeCase;

fuzz_target!(|data: &[u8]| {
    let Some(case) = unityrs_fuzz::Case::new(data) else {
        return;
    };
    let kind = if case.case.has(TreeCase::STREAM) {
        ReaderKind::Stream
    } else {
        ReaderKind::Memory
    };
    unityrs_fuzz::with_stack(|| {
        let read = ReadCtx {
            endian: case.endian(),
            ref_types: case.ref_types(),
            options: &case.options,
            meter: &case.meter,
        };
        let start = match kind {
            ReaderKind::Memory => i128::from(case.case.start),
            ReaderKind::Stream => i128::from(case.case.start.unsigned_abs()),
        };
        let _ = python::read_with_reader(&case.data, start, &case.tree, &read, kind);
    });
});
