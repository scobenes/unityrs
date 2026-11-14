//! Validate arbitrary ZIP directory layouts and bounded member inflation.
#![no_main]
use libfuzzer_sys::fuzz_target;
use unityrs_core::{
    Bytes, RawStr,
    env::{EnvOptions, Environment, Source},
    zip::ZipArchive,
};
fuzz_target!(|data: &[u8]| {
    if data.len() as u64 > unityrs_fuzz::WORK {
        return;
    }
    let ctx = unityrs_fuzz::context();
    let bytes = Bytes::copy_from_slice(data);
    let mut options = EnvOptions::unitypy();
    options.context = ctx.clone();
    let env = Environment::new(options);
    let _ = env.load_zip_file(Source::Bytes {
        name: RawStr::from("fuzz.zip"),
        data: bytes.clone(),
    });
    let meter = unityrs_fuzz::meter(&ctx);
    if let Ok(zip) = ZipArchive::open(bytes, &meter) {
        for i in 0..zip.members().len() {
            if zip.read(i, &ctx.limits).is_err() {
                break;
            }
        }
    }
});
