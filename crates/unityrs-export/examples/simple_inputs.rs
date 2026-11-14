//! Synthetic L7 inputs for `tools/fixtures/simple_export.py`; no TPK input.
#![allow(clippy::format_collect, reason = "test protocol hex encoding")]
#[path = "../tests/support/simple_cases.rs"]
mod cases;
fn main() {
    let cases: Vec<_> = cases::cases().into_iter().map(|case| {
        let input = case.input.iter().map(|b| format!("{b:02x}")).collect::<String>();
        serde_json::json!({"case": case.name, "class_id": case.class_id, "extension": case.extension, "input": input})
    }).collect();
    println!("{}", serde_json::to_string(&cases).unwrap());
}
