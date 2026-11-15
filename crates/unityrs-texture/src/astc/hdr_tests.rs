//! Scalar astcenc HDR/F16 replay before the byte conversion or image flip.
use std::collections::BTreeSet;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;

use super::block;
use crate::astc::context::{Footprint, Tables};

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn hash(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
}

#[test]
fn every_hdr_block_matches_astcenc_binary16_before_scaling() {
    let doc: J = serde_json::from_str(include_str!("../../tests/fixtures/astc_hdr.json")).unwrap();
    assert_eq!(doc["profile"], "HDR");
    assert_eq!(doc["flags"], "DECOMPRESS_ONLY");
    assert_eq!(doc["half_backend"], "scalar");
    let mut total = 0;
    for side in [4_u8, 5, 6, 8, 10, 12] {
        let meter = Meter::unlimited();
        let mut tables = Tables::new(Footprint::new(side, side).unwrap(), &meter).unwrap();
        let mut tags = BTreeSet::new();
        for c in doc["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["side"] == side)
        {
            total += 1;
            let id = c["id"].as_str().unwrap();
            tags.extend(
                c["tags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap().to_owned()),
            );
            let data: [u8; 16] = unhex(c["data"].as_str().unwrap()).try_into().unwrap();
            let mut pixels = [[0; 4]; 144];
            let got = block(&mut tables, &data, &mut pixels);
            if c["raises"] == "InvalidBlock" {
                assert_eq!(got, None, "{id}");
            } else {
                assert_eq!(got, Some(()), "{id}");
                let bytes: Vec<u8> = pixels[..usize::from(side).pow(2)]
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                assert_eq!(hash(&bytes), c["half_sha256"].as_str().unwrap(), "{id}");
            }
        }
        for cem in [2, 3, 7, 11, 14, 15] {
            assert!(
                tags.contains(&format!("cem:{cem}")),
                "side {side}, CEM {cem}"
            );
        }
        for partitions in 1..=4 {
            assert!(
                tags.contains(&format!("partitions:{partitions}")),
                "side {side}, {partitions} partitions"
            );
        }
        for plane in 0..4 {
            assert!(
                tags.contains(&format!("plane2:{plane}")),
                "side {side}, plane {plane}"
            );
        }
        for tag in [
            "single",
            "dual",
            "cem:mixed",
            "error:reserved",
            "error:void_hdr",
            "error:void_reserved",
            "error:void_extent",
        ] {
            assert!(tags.contains(tag), "side {side}, {tag}");
        }
        drop(tables);
        assert_eq!(meter.used(), 0);
    }
    assert_eq!(
        total, 2_393,
        "the full seeded block matrix must be replayed"
    );
}
