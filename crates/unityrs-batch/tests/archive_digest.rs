//! Chunk-independent hashes, with independently computed Python expectations.
use unityrs_batch::digest::{Hashes, Incremental, PART};

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn incremental_matches_slice_at_every_part_edge() {
    for size in [
        0,
        1,
        PART - 1,
        PART,
        PART + 1,
        2 * PART - 1,
        2 * PART,
        2 * PART + 1,
    ] {
        let bytes = vec![b'x'; size];
        let expected = Hashes::of(&bytes);
        for chunk in [1_009, 65_536, PART + 1] {
            let mut digest = Incremental::default();
            for block in bytes.chunks(chunk) {
                digest.update(block).unwrap();
                digest.update(&[]).unwrap();
            }
            let actual = digest.finish();
            assert_eq!(actual.size, u64::try_from(size).unwrap());
            assert_eq!(actual.md5, expected.md5);
            assert_eq!(actual.etag, expected.etag);
        }
    }
}

#[test]
fn one_byte_updates_and_exact_part_have_no_empty_suffix() {
    let mut digest = Incremental::default();
    for _ in 0..PART {
        digest.update(b"x").unwrap();
    }
    let result = digest.finish();
    assert_eq!(result.etag.to_string(), "d4760a6c6500b8c7fbb09e4c65bc558a");
    assert!(!result.etag.is_multipart());
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn python_hashlib_checks_every_boundary_independently() {
    use std::process::Command;
    use unityrs_batch::digest::Hex;
    use unityrs_testkit::{child, scratch::Scratch};
    let mut results = Vec::new();
    let buffer = vec![b'x'; 65_533];
    for size in [
        0,
        1,
        PART - 1,
        PART,
        PART + 1,
        2 * PART - 1,
        2 * PART,
        2 * PART + 1,
    ] {
        let mut digest = Incremental::default();
        let mut left = size;
        while left > 0 {
            let n = left.min(buffer.len());
            digest.update(&buffer[..n]).unwrap();
            left -= n;
        }
        let result = digest.finish();
        results.push(serde_json::json!({"size":result.size,"md5":Hex(&result.md5).to_string(),"etag":result.etag.to_string()}));
    }
    let scratch = Scratch::new("archive-digests");
    let path = scratch.join("digests.json");
    std::fs::write(&path, serde_json::to_vec(&results).unwrap()).unwrap();
    let out = child::output(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/archive_support/check.py"
            ))
            .arg("--digests")
            .arg(path),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
