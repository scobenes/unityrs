//! Resource lookup (`helpers/ResourceReader.py:10-38`).
//!
//! `get_resource_data(res_path, ...)` tries four CAB names, in order:
//! `basename`, `name + ".resource"`, `name + ".assets.resS"`,
//! `name + ".resS"`, where `basename = ntpath.basename(res_path)` and `name`
//! is `ntpath.splitext(basename)[0]`. On a miss it loads the file's externals
//! and the four candidates as dependencies, then tries again, and raises
//! `FileNotFoundError` when all four still miss. The bytes are
//! `reader.read_bytes(size)` from `offset`.
//!
//! Owner: package D.

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use crate::raw::RawStr;

/// The four candidate names, in UnityPy's order.
pub fn resource_candidates(res_path: &[u8]) -> [RawStr; 4] {
    todo!()
}
