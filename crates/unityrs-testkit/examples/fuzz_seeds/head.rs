//! HEAD reader fixtures are synthetic inputs built by `tools/fixtures/head_read.py`.
//! Seed the existing raw-byte targets with every unique input, including refusals.
use super::Seeds;

pub(super) fn write(seeds: &mut Seeds) -> std::io::Result<()> {
    let oracle: serde_json::Value = serde_json::from_str(include_str!(
        "../../../unityrs-core/tests/fixtures/head_read.json"
    ))?;
    for case in oracle["cases"].as_array().expect("HEAD cases") {
        if case["stream"] == true {
            continue;
        }
        let hex = case["hex"].as_str().expect("HEAD bytes");
        let data: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex byte"))
            .collect();
        let target = case["kind"].as_str().expect("HEAD target");
        seeds.add(
            target,
            &format!("head-{}", case["name"].as_str().expect("case name")),
            &data,
        )?;
        if target == "serialized" {
            super::extractor_paths::file(
                seeds,
                &format!("head-{}", case["name"].as_str().expect("case name")),
                &data,
            )?;
        }
    }
    Ok(())
}
