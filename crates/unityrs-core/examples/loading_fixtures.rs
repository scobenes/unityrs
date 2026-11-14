//! Emit testkit-built ZIP/split inputs for tools/fixtures/loading.py.
#[path = "../tests/support/loading.rs"]
mod support;
fn main() {
    println!("{}", support::inputs());
}
