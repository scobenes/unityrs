//! Replay answers recorded by tools/fixtures/unitycn.py in the pinned oracle.
use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_core::budget::Reservation;
use unityrs_core::codec::{BlockDecryptor, unitycn::UnityCnDecryptor};
use unityrs_core::limits::Meter;
use unityrs_core::stream::{Endian, Reader};
use unityrs_core::{BundleFile, Bytes, Error, FileContext, RawStr, Unsupported};
#[path = "support/unitycn.rs"]
#[allow(dead_code)]
mod support;

fn class(error: Error) -> Value {
    json!({"error": match error {
        Error::UnityCnKeyLength { .. } | Error::UnityCnVectorLength { .. } => "ValueError",
        Error::UnityCnKeyMissing => "LookupError",
        Error::UnityCnSignature => "Exception",
        other => panic!("unexpected oracle error: {other}"),
    }})
}

fn key(mode: &str) -> Option<&'static [u8; 16]> {
    match mode {
        "correct" => Some(support::KEY),
        "wrong" => Some(b"synthetic-key-02"),
        "missing" => None,
        _ => panic!("unknown key mode"),
    }
}

#[test]
fn unitycn_matches_the_recorded_oracle() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unitycn/oracle.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!(
        "UnityCN oracle JSON is required: {e}. Run scripts/dev python tools/fixtures/unitycn.py in the pinned oracle container, then commit its output."));
    let oracle: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(oracle["unitypy"], "1.25.2");
    let inputs = support::inputs();
    assert!(
        oracle["inputs"] == inputs,
        "oracle inputs are stale; regenerate with tools/fixtures/unitycn.py in the pinned oracle container"
    );
    for (name, count) in [
        ("bundles", inputs["bundles"].as_array().unwrap().len() * 3),
        (
            "headers",
            inputs["header_lengths"].as_array().unwrap().len() * 3,
        ),
        ("blocks", inputs["blocks"].as_array().unwrap().len()),
        ("setters", 5),
        ("vectors", 2),
    ] {
        assert_eq!(oracle[name].as_array().unwrap().len(), count, "{name}");
    }
    vectors(&oracle, &inputs);
    headers_and_setters(&oracle, &inputs);
    bundles(&oracle, &inputs);
    blocks(&oracle, &inputs);
}

fn vectors(oracle: &Value, inputs: &Value) {
    let header = support::unhex(inputs["header"].as_str().unwrap());
    let mut nibbles = support::INDEX.to_vec();
    for i in 0..4 {
        for j in 0..4 {
            nibbles.push(support::SUBSTITUTE[j * 4 + i]);
        }
    }
    let tables: Vec<u8> = nibbles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[a, b]| a << 4 | b)
        .collect();
    for (row, at, plain) in [
        (&oracle["vectors"][0], 4, tables.as_slice()),
        (&oracle["vectors"][1], 37, b"#$unity3dchina!@".as_slice()),
    ] {
        assert_eq!(row["data"], support::hex(&header[at..at + 16]));
        assert_eq!(row["key"], support::hex(&header[at + 16..at + 32]));
        assert_eq!(row["position"], at + 33);
        assert_eq!(row["decrypted"], support::hex(plain));
    }
}

fn headers_and_setters(oracle: &Value, inputs: &Value) {
    for row in oracle["setters"].as_array().unwrap() {
        let length = usize::try_from(row["length"].as_u64().unwrap()).unwrap();
        let result = FileContext::default().set_assetbundle_decrypt_key(vec![0; length]);
        assert_eq!(
            result.map_or_else(class, |()| json!({"value":null})),
            row["result"]
        );
    }
    for row in oracle["headers"].as_array().unwrap() {
        let header = support::unhex(inputs["header"].as_str().unwrap());
        let length = usize::try_from(row["length"].as_u64().unwrap()).unwrap();
        let mut reader = Reader::new(&header[..length], Endian::Big);
        let result = UnityCnDecryptor::read(&mut reader, key(row["key"].as_str().unwrap()));
        assert_eq!(
            result.map_or_else(class, |_| json!({"value":null})),
            row["result"],
            "{row}"
        );
        assert_eq!(json!(reader.position()), row["position"]);
    }
}

fn bundles(oracle: &Value, inputs: &Value) {
    for row in oracle["bundles"].as_array().unwrap() {
        let case = inputs["bundles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == row["name"])
            .unwrap();
        let data = support::unhex(case["hex"].as_str().unwrap());
        let mut ctx = FileContext::default();
        if let Some(key) = key(row["key"].as_str().unwrap()) {
            ctx.set_assetbundle_decrypt_key(key).unwrap();
        }
        let meter = Meter::new(
            ctx.limits.max_input_memory,
            ctx.budget.clone(),
            Reservation::Input,
        );
        let result = BundleFile::open(
            RawStr::from("synthetic.bundle"),
            Bytes::from(data),
            None,
            &meter,
            &Arc::new(ctx),
        )
        .and_then(|bundle| {
            bundle.load_all()?;
            if case["path"].is_array() {
                let unityrs_core::File::Bundle(inner) = bundle.entry(0)? else {
                    panic!("nested fixture must contain a bundle");
                };
                inner.entry_bytes(0)
            } else {
                bundle.entry_bytes(0)
            }
        });
        let got = result.map_or_else(class, |bytes| json!({"value":support::hex(&bytes)}));
        assert_eq!(got, row["result"], "{row}");
    }
}

fn blocks(oracle: &Value, inputs: &Value) {
    for row in oracle["blocks"].as_array().unwrap() {
        let case = inputs["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == row["name"])
            .unwrap();
        let index = support::unhex(case["index"].as_str().unwrap())
            .try_into()
            .unwrap();
        let sub = support::unhex(case["substitute"].as_str().unwrap())
            .try_into()
            .unwrap();
        let header = support::header(&index, &sub);
        let dec =
            UnityCnDecryptor::read(&mut Reader::new(&header, Endian::Big), Some(support::KEY))
                .unwrap();
        let mut block = support::unhex(case["hex"].as_str().unwrap());
        let got = dec.decrypt(
            &mut block,
            usize::try_from(case["block_index"].as_u64().unwrap()).unwrap(),
        );
        if case["tier_u"] == true {
            assert!(row["boost"].is_null());
            assert_eq!(row["python"]["error"], "IndexError");
            assert!(matches!(
                got,
                Err(Error::Unsupported(Unsupported::UnityCnWalk))
            ));
        } else {
            got.unwrap();
            assert_eq!(json!({"value":support::hex(&block)}), row["boost"]);
        }
    }
}
