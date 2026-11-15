//! UnityWeb/UnityRaw headers, levels, LZMA and directory entries.
#![no_main]
use libfuzzer_sys::fuzz_target;
use unityrs_core::{BundleFile, Bytes, RawStr, stream::ReaderKind};
fuzz_target!(|data: &[u8]| {
    if data.len() as u64 > unityrs_fuzz::WORK {
        return;
    }
    let ctx = unityrs_fuzz::context();
    let bytes = Bytes::copy_from_slice(data);
    for reader in [ReaderKind::Memory, ReaderKind::Stream] {
        let meter = unityrs_fuzz::meter(&ctx);
        if let Ok(bundle) = BundleFile::open_with_reader(
            RawStr::from("fuzz.bundle"),
            bytes.clone(),
            None,
            &meter,
            &ctx,
            reader,
        ) {
            let _ = bundle.load_all();
        }
    }
});
