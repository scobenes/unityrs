//! Every synthetic bundle-save oracle input also reaches the roundtrip harness.
use super::Seeds;
use std::collections::BTreeSet;

pub(super) fn write(seeds: &mut Seeds) -> std::io::Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../unityrs-core/tests/fixtures/bundle_save.json"
    ))?;
    let mut seen = BTreeSet::new();
    for case in fixture["cases"].as_array().expect("bundle cases") {
        let index = usize::try_from(case["input"].as_u64().expect("input index")).expect("usize");
        if !seen.insert(index) {
            continue;
        }
        let mut data = Vec::new();
        for chunk in fixture["bytes"][index].as_array().expect("byte chunks") {
            let part = usize::try_from(chunk.as_u64().expect("chunk index")).expect("usize");
            let hex = fixture["data"][part].as_str().expect("chunk hex");
            for pair in hex.as_bytes().as_chunks::<2>().0 {
                data.push(
                    u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("byte"),
                );
            }
        }
        seeds.add(
            "bundle_save",
            case["name"].as_str().expect("case name"),
            &data,
        )?;
    }
    Ok(())
}
