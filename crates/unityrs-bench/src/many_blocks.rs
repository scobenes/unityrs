//! `many-blocks.bundle` (plan section 2): one entry of 64 MiB (1 MiB at the
//! gate scale) in 16 KiB LZ4 blocks, so loading it is block decoding. The
//! entry is a resource, not a `SerializedFile`: a fixed ASCII header, then
//! 16-byte runs, each either seeded bytes or a copy of a run 256 bytes back,
//! which LZ4 takes to about half.

use serde_json::json;
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::TestCodec;

use crate::{Fixture, Scale, UNITY_VERSION, bundle, seed_of, sha256_hex};

/// The file's name.
pub const NAME: &str = "many-blocks.bundle";

/// The entry's name in the bundle.
pub const ENTRY: &str = "CAB-bench-many-blocks.resS";

/// The LZ4 block size.
pub const BLOCK: usize = 16 << 10;

/// The entry's header: never a signature or magic any reader detects.
const HEADER: &[u8] = b"unityrs-bench many-blocks\0";

/// The entry's bytes at `scale`.
#[must_use]
pub fn entry(scale: Scale) -> Vec<u8> {
    let len = scale.pick(64usize << 20, 1 << 20);
    let mut rng = Rng::new(seed_of("many-blocks"));
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(HEADER);
    while out.len() < len {
        let back = out.len().checked_sub(256);
        match back {
            Some(at) if rng.below(2) == 0 => {
                let run: Vec<u8> = out.get(at..at + 16).unwrap_or_default().to_vec();
                out.extend_from_slice(&run);
            }
            _ => {
                out.extend_from_slice(&rng.next_u64().to_le_bytes());
                out.extend_from_slice(&rng.next_u64().to_le_bytes());
            }
        }
    }
    out.truncate(len);
    out
}

/// The bundle at `scale`; its inventory names the entry's size and sha256,
/// the workload's digest.
#[must_use]
pub fn build(scale: Scale) -> Fixture {
    let data = entry(scale);
    let inventory = json!({
        "kind": "many-blocks",
        "entry": ENTRY,
        "entry_bytes": data.len(),
        "entry_sha256": sha256_hex(&data),
        "blocks": data.len().div_ceil(BLOCK),
    });
    let bytes = bundle(
        UNITY_VERSION,
        vec![(ENTRY.to_owned(), data, 0)],
        TestCodec::Lz4,
        BLOCK,
    );
    Fixture {
        name: NAME.to_owned(),
        bytes,
        inventory,
    }
}
