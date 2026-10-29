//! Synthetic Unity files built in code, for unityrs tests.
//!
//! Everything here is made from scratch by the builders: never game data,
//! never bytes copied from a real file. The builders return `Vec<u8>` and
//! know nothing of `unityrs-core`, so core's own tests can use them.
//!
//! Owners (docs/plans/core.md section 7): [`writer`] is the scaffold's
//! (implemented); [`blob`], [`serialized`] and [`bundle`] are package C's;
//! [`env`](mod@env) is package D's.

pub mod blob;
pub mod bundle;
pub mod env;
pub mod serialized;
pub mod writer;

pub use writer::ByteWriter;
