//! Bundle-save inputs and HEAD's saved offset/padding layouts reach the harness.
use super::Seeds;
use std::collections::BTreeSet;

pub(super) fn write(seeds: &mut Seeds) -> std::io::Result<()> {
    for text in [
        include_str!("../../../unityrs-core/tests/fixtures/bundle_save.json"),
        include_str!("../../../unityrs-core/tests/fixtures/head_save_bundle.json"),
    ] {
        write_fixture(seeds, text)?;
    }
    Ok(())
}

fn write_fixture(seeds: &mut Seeds, text: &str) -> std::io::Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(text)?;
    let mut seen = BTreeSet::new();
    let fields: &[&str] = if fixture.get("head").is_some() {
        &["input", "output"]
    } else {
        &["input"]
    };
    for case in fixture["cases"].as_array().expect("bundle cases") {
        for field in fields {
            let index = usize::try_from(case[field].as_u64().expect("byte index")).expect("usize");
            if !seen.insert(index) {
                continue;
            }
            let mut data = Vec::new();
            for chunk in fixture["bytes"][index].as_array().expect("byte chunks") {
                let part = usize::try_from(chunk.as_u64().expect("chunk index")).expect("usize");
                let hex = fixture["data"][part].as_str().expect("chunk hex");
                for pair in hex.as_bytes().as_chunks::<2>().0 {
                    data.push(
                        u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16)
                            .expect("byte"),
                    );
                }
            }
            seeds.add(
                "bundle_save",
                &format!("{}-{field}", case["name"].as_str().expect("case name")),
                &data,
            )?;
        }
    }
    Ok(())
}
