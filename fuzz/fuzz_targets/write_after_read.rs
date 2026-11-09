//! Write after read (`docs/DESIGN.md`'s write rule: semantic equality plus
//! idempotence): a value read from a fuzzed tree, ref types and bytes
//! (`unityrs_testkit::fuzz::TreeCase`), by UnityPyBoost's reader or, with
//! `TreeCase::PYTHON`, the pure-Python one from position 0, is written
//! under `WriteRules::MirrorReader` (write as the reader that built the
//! value reads). The writer may refuse a value, with its named error
//! (`WriteError`), never a panic. What it writes, read again by the same
//! reader, is the same value, and the read ends at the end of what was
//! written; written again, it is the same bytes. A UnityPyBoost read wrote
//! back exactly the bytes it read. A tree that repeats a name is only
//! read and written: its dict keeps the last value of the name.
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::error::LimitKind;
use unityrs_core::walk::{
    ReadCtx, ReadDetail, ReadError, WriteCtx, WriteRules, boost, python, write,
};
use unityrs_core::{Bytes, Parsed};
use unityrs_fuzz::Case;
use unityrs_testkit::fuzz::TreeCase;

fn read(
    case: &Case,
    data: &Bytes,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> Result<Parsed, ReadError> {
    let ctx = ReadCtx {
        endian: case.endian(),
        ref_types: case.ref_types(),
        options: &case.options,
        meter,
    };
    if case.case.has(TreeCase::PYTHON) {
        python::read(data, 0, &case.tree, &ctx)
    } else {
        boost::read(data, &case.tree, &ctx)
    }
}

/// `value` written; `None` when the writer refused it or a limit stopped it.
fn write_out(
    case: &Case,
    value: &Parsed,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> Option<Bytes> {
    let ctx = WriteCtx {
        endian: case.endian(),
        ref_types: case.ref_types(),
        rules: WriteRules::MirrorReader,
        limits: &case.options.limits,
    };
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory).ok()?;
    write::write(value, &case.tree, &ctx, &mut out).ok()?;
    Some(Bytes::copy_from_slice(out.as_slice()))
}

fuzz_target!(|data: &[u8]| {
    let Some(case) = Case::new(data) else {
        return;
    };
    unityrs_fuzz::with_stack(|| {
        let Ok(first) = read(&case, &case.data, &case.meter) else {
            return;
        };
        let Some(written) = write_out(&case, &first, &case.meter) else {
            return;
        };
        if !case.names_unique() {
            return;
        }
        // A fresh meter: the first read and write have used the open's work.
        let again = case.new_meter();
        let second = match read(&case, &written, &again) {
            Ok(p) => p,
            // The same bounds may stop a read of what was written (its
            // bytes can be longer than what the first read read).
            Err(ReadError {
                detail: ReadDetail::Limit(_) | ReadDetail::Charge(_),
                ..
            }) => return,
            Err(e) => panic!("what was written does not read back: {e}\nwritten {written:02x?}"),
        };
        assert!(
            second.value() == first.value(),
            "read back as another value\nfirst {:?}\nsecond {:?}\nwritten {written:02x?}",
            first.value(),
            second.value()
        );
        let len = i64::try_from(written.len()).unwrap_or(i64::MAX);
        assert_eq!(
            second.bytes_read, len,
            "the read back ends before what was written ends"
        );
        if !case.case.has(TreeCase::PYTHON) {
            assert_eq!(
                first.bytes_read, len,
                "UnityPyBoost's read is not written back whole"
            );
        }
        let Some(rewritten) = write_out(&case, &second, &again) else {
            panic!("the value read back does not write");
        };
        assert_eq!(rewritten, written, "a second write writes other bytes");
    });
});
