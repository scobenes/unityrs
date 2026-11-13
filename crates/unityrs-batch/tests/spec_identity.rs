use serde_json::json;
use std::fs;
use unityrs_batch::spec;
use unityrs_testkit::scratch::Scratch;
fn job(spool: &std::path::Path, sink: &std::path::Path) -> serde_json::Value {
    json!({"spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,"dep_cache_bytes":33_554_432,
        "spool":{"dir":spool,"size_bytes":1_073_741_824,"reserve_bytes":10},
        "inputs":[{"id":"a","path":"/source"}],"outputs":{"textures":{"key":"{input}/{name}.png"}},"sink":{"kind":"directory","root":sink}})
}
#[test]
fn spec_digest_is_from_the_parsed_buffer_and_survives_preparation() {
    use sha2::{Digest, Sha256};
    let scratch = Scratch::new("local-digest");
    let sink = Scratch::new("local-digest-sink");
    let mut bytes = serde_json::to_vec(&job(scratch.path(), sink.path())).unwrap();
    bytes.extend_from_slice(b" \n");
    let expected: [u8; 32] = Sha256::digest(&bytes).into();
    let path = scratch.join("job.json");
    fs::write(&path, &bytes).unwrap();
    let validated = spec::load(&path).unwrap();
    fs::write(&path, b"changed").unwrap();
    assert_eq!(validated.spec_sha256, expected);
    let prepared = unityrs_batch::phase1::prepare(validated).unwrap();
    assert_eq!(prepared.spec_sha256, expected);
    let changed: [u8; 32] = Sha256::digest(b"changed").into();
    assert_ne!(changed, expected);
}
