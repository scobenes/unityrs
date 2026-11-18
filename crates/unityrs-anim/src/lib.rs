//! Clip discovery and binding to a Unity scene graph.
//!
//! Callers supply the loaded view, meter and renderer facts. Discovery yields
//! rig-scoped clip identities and retains serialized slots and failure causes.
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

mod bindings;
mod controller;
mod discovery;
mod error;
mod read;
mod targets;

pub use bindings::{
    BindingKind, BindingLayout, BindingTable, GenericBinding, SlotBinding, SlotCounts,
    TransformProperty, ValueFlags, read_bindings,
};
pub use targets::{
    Attachment, BindingTarget, MorphProperty, PathQuery, RendererChannel, RendererChannels,
    RigPaths,
};

pub use controller::{ControllerLeaf, ControllerTable};
pub use discovery::{
    ClipSelector, ClipSlot, Discovery, RigClips, SelectionError, SlotError, SourceSlot, discover,
};
pub use error::AnimError;

const fn send_sync<T: Send + Sync + ?Sized>() {}

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
