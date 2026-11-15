//! Emit synthetic inputs for tools/fixtures/web.py's texture-fetch oracle.
#[path = "../tests/support/fetch.rs"]
mod support;

fn main() {
    println!("{}", serde_json::json!(support::inputs()));
}
