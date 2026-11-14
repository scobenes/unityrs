// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py's Mesh data model. See THIRD_PARTY_LICENSES.
//! Owned Unity mesh arrays and metadata, in Unity's space, without normalisation.
//!
//! [`read_mesh`] parses the object through the core and runs meshdecode's
//! handler and triangle reader. The numeric arrays move into [`ProcessedOwned`]
//! without copying; metadata is copied into independent, charged owners.
//! Every input-sized buffer is a [`unityrs_core::limits::MeteredVec`], charged
//! before fallible allocation, with work counted before each scan or copy.
//! Public buffers retain their own charges even when moved out of the model.
//!
//! The core parses on the source file's meter; output and decoding use the
//! supplied meter. Callers normally pass the file's own meter. No reservation
//! occurs inside a panic-containment boundary; this reader runs on the caller's
//! thread. Variable bone-count weights and scene/export policy are outside this
//! model (docs/plans/static3d.md, decisions 8 and 9).
//!
//! Refusal audit: geometry and triangle failures pass through meshdecode;
//! `tests/oracle.rs` checks their exception classes and messages against the
//! pinned UnityPy and corrected answers, as well as every successful array.
//! The existing decoder's documented P4, Tier U, wrapped-access and metering
//! boundaries still apply. Core parse/resource errors and non-Mesh objects
//! are rejected before metadata is read. Metadata checks only require the
//! scalar types, widths and record fields of the supported Unity layouts
//! (`classes/generated.py`); they impose no matrix-completeness, finite-value,
//! index-range, frame-range or LOD-consistency policy. Deliberately mistyped
//! metadata trees are outside that typed model: UnityPy's attrs constructors
//! do not validate annotations, so a metadata shape refusal is not claimed as
//! a matching Python exception. `tests/limits.rs` pins this Tier U boundary
//! separately from the oracle failures and memory/work/allocator refusals.
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

mod fields;
mod model;
pub mod obj;
pub mod pyfmt;
mod read;
mod shapes;

pub use model::*;
pub use read::{read_mesh, read_mesh_for_obj};
pub use unityrs_meshdecode::{ProcessedOwned, Triangles};

/// A mesh could not be read. Errors retain no input-sized diagnostic text.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MeshError {
    /// The object is not a Mesh.
    #[error("object is not a Mesh")]
    NotMesh,
    /// The core could not parse the object.
    #[error(transparent)]
    Parse(#[from] unityrs_core::walk::ParseError),
    /// A source-file or resource error.
    #[error(transparent)]
    Core(#[from] unityrs_core::Error),
    /// The geometry handler refused the input.
    #[error(transparent)]
    Decode(#[from] unityrs_meshdecode::MeshError),
    /// A metadata field is missing, has the wrong type or is out of range.
    #[error("malformed mesh field: {0}")]
    Field(&'static str),
    /// Renderer or component access failed.
    #[error(transparent)]
    Scene(#[from] unityrs_scene::SceneError),
    /// A material or texture pointer failed to resolve.
    #[error(transparent)]
    Deref(#[from] unityrs_core::env::DerefError),
    /// Texture decoding refused the input.
    #[error(transparent)]
    Texture(#[from] unityrs_texture::DecodeError),
    /// Export output failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Object-mode view refused a field.
    #[error(transparent)]
    View(#[from] unityrs_core::objmode::ViewError),
    /// A memory, budget, allocator or work refusal.
    #[error(transparent)]
    Charge(#[from] unityrs_core::limits::ChargeError),
}
const _: () = send_sync::<MeshError>();
const fn send_sync<T: Send + Sync + ?Sized>() {}

#[cfg(test)]
mod tests {
    #[test]
    fn every_public_type_is_asserted() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }
    #[test]
    fn the_reader_starts_no_thread_and_never_waits() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "waits or starts a thread: {found:?}");
    }
}
