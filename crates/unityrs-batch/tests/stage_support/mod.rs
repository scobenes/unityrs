#![allow(dead_code)]
use serde_json::{Value, json};
use std::path::Path;
use unityrs_batch::spec::{self, Validated};
pub fn spec(path: &Path, size: Option<u64>, sha: Option<&str>) -> Validated {
    let mut bundle = json!({"id":"x", "path":path});
    if let Some(size) = size {
        bundle["size"] = size.into();
    }
    if let Some(sha) = sha {
        bundle["sha256"] = sha.into();
    }
    from_bundles(&[bundle], &[])
}
pub fn from_bundles(inputs: &[Value], dependencies: &[Value]) -> Validated {
    spec::parse(
        serde_json::to_vec(&json!({
            "spec_version":1,"run_id":"run","attempt":1,"threads":2,
            "download_concurrency":2,"max_inflight_uncompressed_bytes":67_108_864,
            "max_decoded_bytes":16_777_216,"dep_cache_bytes":16_777_216,
            "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
            "inputs":inputs,"dependencies":dependencies,
            "outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/out"}
        }))
        .unwrap(),
    )
    .unwrap()
}

pub fn run(
    copy: unityrs_batch::stage::local::LocalCopy,
    bundle: u32,
    destination: &mut unityrs_batch::spool::OwnedFile,
    stopped: &dyn Fn() -> bool,
    hooks: &dyn unityrs_batch::stage::local::Hooks,
) -> unityrs_batch::stage::StageEnd {
    let pending = copy.run(bundle, destination, stopped, hooks);
    pending.finish(destination, stopped, hooks)
}
