//! Shared synthetic loading inputs, emitted for the Python oracle recorder.
#![allow(dead_code)]
use serde_json::{Value, json};
use unityrs_testkit::env::{Obj, bundle, serialized};
use unityrs_testkit::zip::archive;

pub fn inputs() -> Value {
    let asset = serialized(&[], &[Obj::named(7, "synthetic-λ", &[])]);
    let nested = bundle(&[("CAB-N", asset.clone())]);
    let half = asset.len() / 2;
    let scenes: Vec<_> = [
        ("stored", vec![("CAB-A", asset.as_slice(), false)]),
        ("deflate", vec![("dir/CAB-A", asset.as_slice(), true)]),
        ("mixed", vec![("folder/", &b""[..], false), ("raw.resS", &b"raw"[..], true), ("nested", nested.as_slice(), true)]),
        ("duplicates", vec![("raw", &b"first"[..], false), ("raw", &b"last"[..], true)]),
        ("archive-split", vec![("CAB-A.split1", &asset[half..], true), ("CAB-A.split0", &asset[..half], false)]),
        ("missing-zero", vec![("CAB-A.split1", asset.as_slice(), true)]),
    ].into_iter().map(|(name, entries)| {
        json!({"name": name, "zip": hex(&archive(&entries)), "entries": entries.iter().map(|(n,b,d)| json!({"name":n,"hex":hex(b),"deflate":d})).collect::<Vec<_>>()})
    }).collect();
    json!({"scenes":scenes, "asset":hex(&asset)})
}

pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
