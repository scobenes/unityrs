//! Synthetic Unity files built in code, for unityrs tests.
//!
//! Everything here is made from scratch by the builders: never game data,
//! never bytes copied from a real file. The builders return `Vec<u8>` and
//! know nothing of `unityrs-core`, so core's own tests can use them.
//!
//! Owners (docs/plans/core.md section 7): [`writer`], [`rng`],
//! [`public_types`] (the `Send + Sync` source scan the crates' tests share)
//! and [`waits`] (the no-thread, no-wait source scan) are the scaffold's
//! (implemented); [`blob`], [`serialized`] and [`bundle`] are package C's;
//! [`env`](mod@env) is package D's; [`depth`] (the deep values of plan
//! section 3.10), [`counted`] (the counting global allocator, and the
//! thread it counts), [`guard`] (the deadlock guard of every test that
//! hands work between threads), [`stack`] (the child-process probes that
//! measure a stack, and tell an overflow from every other failure) and
//! [`child`] (how every test runs a child process: a `SIGKILL` from
//! outside is no result) are package B's; [`address_space`] (the cap that makes the C allocator
//! refuse, for the tests of liblzma's own allocations) is package C's;
//! [`mutate`] (G3b's mutator) is the integration's.

pub mod address_space;
pub mod blob;
pub mod bundle;
pub mod child;
pub mod counted;
pub mod depth;
pub mod env;
pub mod guard;
pub mod mutate;
pub mod public_types;
pub mod rng;
pub mod serialized;
pub mod stack;
pub mod waits;
pub mod writer;

pub use rng::Rng;
pub use writer::ByteWriter;
