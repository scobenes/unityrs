//! `SerializedFile::parse` over any bytes, by both readers' rules, then
//! every object read as the dump reads it (`parse_as_dict`, UnityPyBoost's
//! reader or the pure-Python one by the object's size, with `check_read`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::files::SerializedFile;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, RawStr};

fuzz_target!(|data: &[u8]| {
    let ctx = unityrs_fuzz::context();
    let data = Bytes::copy_from_slice(data);
    unityrs_fuzz::with_stack(|| {
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            let meter = unityrs_fuzz::meter(&ctx);
            let name = RawStr::from("CAB-fuzz");
            if let Ok(f) =
                SerializedFile::parse_with_reader(name, data.clone(), None, &meter, &ctx, kind)
            {
                for o in f.objects() {
                    let _ = o.parse_as_dict(true);
                }
            }
        }
    });
});
