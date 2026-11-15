//! Shared synthetic WebFile save inputs, also consumed by the pinned oracle.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_core::files::{File, WebFile, WebPacker, WebSaveOptions};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::web;

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}
pub fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
pub fn load(bytes: Bytes, meter: &Arc<Meter>) -> Arc<WebFile> {
    WebFile::open(
        RawStr::from_static(b"save.web"),
        bytes,
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}
pub fn packer(s: &str) -> WebPacker {
    match s {
        "none" => WebPacker::None,
        "gzip" => WebPacker::Gzip,
        "brotli" => WebPacker::Brotli,
        _ => panic!("packer {s}"),
    }
}
pub fn inputs() -> Vec<Value> {
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "synthetic-λ", &[])],
    );
    let nested = web::plain("TuanjieWebData1.0", &[("asset", &asset)]);
    let bundle = unityrs_testkit::env::bundle(&[("raw.resS", b"bundle".to_vec())]);
    let mut inputs = vec![];
    for signature in ["UnityWebData1.0", "TuanjieWebData1.0"] {
        for (label, entries) in [
            ("empty", vec![]),
            (
                "raw",
                vec![("empty", &b""[..]), ("λ/路径.resS", &b"payload\0\xff"[..])],
            ),
            (
                "duplicate",
                vec![
                    ("raw", &b"first"[..]),
                    ("a\\b", &b"middle"[..]),
                    ("raw", &b"last"[..]),
                ],
            ),
            (
                "nested",
                vec![("inner", nested.as_slice()), ("bundle", bundle.as_slice())],
            ),
        ] {
            add(
                &mut inputs,
                &format!("{signature}-{label}"),
                &web::plain(signature, &entries),
            );
        }
    }
    // Cross both gzip and brotli stored-block boundaries with one shared payload.
    let payload: Vec<u8> = (0..65536).map(|i| u8::try_from(i % 251).unwrap()).collect();
    add(
        &mut inputs,
        "block-boundary",
        &web::plain("UnityWebData1.0", &[("raw.resS", &payload)]),
    );
    inputs
}
fn add(inputs: &mut Vec<Value>, label: &str, plain: &[u8]) {
    for (compression, input) in [
        ("none", plain.to_vec()),
        ("gzip", web::gzip(plain)),
        ("brotli", web::brotli(plain)),
    ] {
        for mutation in if label.ends_with("-nested") {
            &["none", "raw", "empty"][..]
        } else {
            &["none"][..]
        } {
            inputs.push(json!({"name":format!("{label}/{compression}/{mutation}"),
                "mutation":mutation,"input":hex(&input)}));
        }
    }
}
pub fn outputs() -> Value {
    let mut cases = vec![];
    for input in inputs() {
        let web = load(
            Bytes::from(unhex(input["input"].as_str().unwrap())),
            &Meter::unlimited(),
        );
        let _held = mutate(&web, input["mutation"].as_str().unwrap());
        for signature in ["UnityWebData1.0", "TuanjieWebData1.0"] {
            for name in ["none", "gzip", "brotli"] {
                let output = web
                    .save_with_signature(
                        WebSaveOptions {
                            packer: packer(name),
                            ..WebSaveOptions::default()
                        },
                        signature,
                    )
                    .unwrap();
                cases.push(json!({"name":input["name"],"input":input["input"],
                    "signature":signature,"packer":name,"mutation":input["mutation"],"rust":hex(&output)}));
            }
        }
    }
    json!(cases)
}

pub fn mutate(web: &WebFile, mutation: &str) -> Option<unityrs_core::ObjectRef> {
    if mutation == "none" {
        return None;
    }
    let File::Web(inner) = web.entry(0).unwrap() else {
        panic!("nested WebFile")
    };
    let File::Serialized(sf) = inner.entry(0).unwrap() else {
        panic!("nested SerializedFile")
    };
    let object = sf.object(7).unwrap();
    object
        .set_raw_data(if mutation == "empty" {
            &b""[..]
        } else {
            &b"edited\0\xff"[..]
        })
        .unwrap();
    Some(object)
}
