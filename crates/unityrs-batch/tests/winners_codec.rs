//! Independent encoding of private frames exercises semantic corruption.
use unityrs_batch::{
    phase::Error,
    sort::Order,
    winners::{codec, stream::By},
};

fn frame(input: &str, kind: u8, mode: u8, hash: bool) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 0, kind, mode, 0];
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    for number in [0_u64, 1, 0] {
        bytes.extend_from_slice(&number.to_le_bytes());
    }
    bytes.extend_from_slice(&1_i64.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&[1; 16]);
    bytes.push(u8::from(hash));
    if hash {
        bytes.extend_from_slice(&[2; 32]);
    }
    bytes.push(0); // no multipart ETag
    for text in ["k", "", input, "CAB-synthetic"] {
        bytes.extend_from_slice(&u16::try_from(text.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
    }
    bytes
}

#[test]
fn candidate_kind_mode_hash_matrix_rejects_semantic_corruption() {
    for kind in 0..=4 {
        for mode in 0..=4 {
            for hash in [false, true] {
                let bytes = frame("a", kind, mode, hash);
                let valid = if kind == 2 {
                    mode == 0 && !hash
                } else {
                    kind < 4 && mode > 0 && hash
                };
                if valid {
                    let row = codec::decode(&bytes).unwrap();
                    assert_eq!(
                        (row.kind, row.mode, row.visible_hash.is_some()),
                        (kind, mode, hash)
                    );
                    By::Key.validate(&bytes).unwrap();
                } else {
                    assert_eq!(
                        codec::decode(&bytes).unwrap_err(),
                        Error::internal(),
                        "kind={kind} mode={mode} hash={hash}"
                    );
                    assert_eq!(By::Key.validate(&bytes), Err(Error::internal()));
                }
            }
        }
    }
}

#[test]
fn non_candidate_markers_keep_their_null_pixel_fields() {
    let bytes = frame("", 0, 0, false);
    let row = codec::decode(&bytes).unwrap();
    assert_eq!(row.input, "");
    assert_eq!(row.visible_hash, None);
    By::Key.validate(&bytes).unwrap();
}
