//! Bounded, metered `GameObject`/Transform graphs in Unity coordinates.
//!
//! Facts use dict fields, with no object-mode name cleaning. Scene roots
//! follow the serialized object table; child and component lists retain
//! their stored order. Every owned array carries its charge, including
//! arrays moved out of a node. Calls run on the caller's thread.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

pub mod accessors;
pub mod builtin;
mod components;
mod error;
mod graph;
mod model;
mod read;
mod visited;

pub use error::SceneError;
pub use graph::{graph_from_game_object, graph_from_scene_file};
pub use model::*;

pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

#[cfg(test)]
mod tests {
    #[test]
    fn public_types_and_no_waits() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "missing Send + Sync: {missing:?}");
        let waits = unityrs_testkit::waits::waits(&src);
        assert!(waits.is_empty(), "waits: {waits:?}");
    }
}
