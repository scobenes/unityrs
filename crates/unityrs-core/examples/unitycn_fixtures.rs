//! Emits synthetic UnityCN oracle inputs; requires no key or input file.
//! Includes nested encrypted bundles with distinct parent/child tables.
#[path = "../tests/support/unitycn.rs"]
#[allow(dead_code)]
mod support;
fn main() {
    println!("{}", support::inputs());
}
