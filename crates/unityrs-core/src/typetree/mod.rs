//! Typetrees: stored rows, UnityPy's tree-building rule, the parsers that read
//! rows from a file, and the compiled [`Tree`] the walker runs on.
//!
//! Owners: `rows.rs` the scaffold; `parse.rs` package C; `commonstring.rs`
//! package A (generated); `tree.rs` package B. This file is frozen.

pub mod commonstring;
pub mod parse;
pub mod rows;
pub mod tree;

pub use rows::{NodeId, NodeList, NodeRow, TreeError};
pub use tree::{Kind, NodeInfo, Shape, Tree};
