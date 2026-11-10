//! `BundleFile::open` and UnityPy's eager load (`load_all`: every block
//! decoded, every entry parsed, nested bundles loaded) over any bytes, by
//! both readers' rules: the memory reader's (an entry, or bytes given) and
//! the stream reader's (a file loaded from a path, as the dump loads one).
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::files::BundleFile;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, RawStr};

fuzz_target!(|data: &[u8]| {
    let ctx = unityrs_fuzz::context();
    let data = Bytes::copy_from_slice(data);
    unityrs_fuzz::with_stack(|| {
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            let meter = unityrs_fuzz::meter(&ctx);
            let name = RawStr::from("fuzz.bundle");
            if let Ok(b) =
                BundleFile::open_with_reader(name, data.clone(), None, &meter, &ctx, kind)
            {
                let _ = b.load_all();
            }
        }
    });
});
