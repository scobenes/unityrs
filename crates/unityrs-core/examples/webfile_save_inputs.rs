//! Synthetic WebFile inputs and Rust saves for bidirectional oracle validation.
#![allow(clippy::unwrap_used)]
#[path = "../tests/support/webfile_save.rs"]
mod support;
fn main() {
    println!("{}", support::outputs());
}
