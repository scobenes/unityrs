//! WebFile tables, nested member loading and metered gzip/brotli inflation.
#![no_main]
use libfuzzer_sys::fuzz_target;
use unityrs_core::{
    Bytes, RawStr, WebFile,
    env::{EnvOptions, Environment, Source},
};
fuzz_target!(|data: &[u8]| {
    if data.len() as u64 > unityrs_fuzz::WORK {
        return;
    }
    let ctx = unityrs_fuzz::context();
    let meter = unityrs_fuzz::meter(&ctx);
    let bytes = Bytes::copy_from_slice(data);
    if let Ok(web) = WebFile::open(RawStr::from("fuzz.data"), bytes.clone(), None, &meter, &ctx) {
        let _ = web.load_all();
    }
    let mut options = EnvOptions::unitypy();
    options.context = ctx;
    let env = Environment::new(options);
    let _ = env.load_file(Source::Bytes {
        name: RawStr::from("fuzz.data"),
        data: bytes,
    });
});
