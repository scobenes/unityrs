// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/MeshHelper.py's MeshHandler, UnityPy/helpers/PackedBitVector.py and UnityPyBoost/Mesh.cpp's unpack_vertexdata, the vertex-data unpack sprites and meshes share. See THIRD_PARTY_LICENSES.
//! A Mesh's or a sprite's vertex arrays, index buffer and triangles, as
//! UnityPy 1.25.2's `MeshHandler` gives them (`docs/plans/extract.md` E16;
//! `PARITY.md` section 15.3's rows marked P1b): the same values, the same
//! types (`float` as `f64`, `int`), and the same exceptions where it
//! raises.
//!
//! - [`Source`]: what the handler reads, a Mesh object's value or a
//!   Sprite's render data, from the core's typetree read, viewed as
//!   UnityPy's object mode builds it (`view`'s module docs).
//! - [`MeshHandler::process`]: the channels and streams by version, a
//!   Mesh's `m_StreamData` through [`Resources`], the index size and
//!   buffer, and each channel's data, through the three `VertexFormat`
//!   tables ([`get_channel_dtype`]) and UnityPyBoost's strided gather
//!   ([`unpack_vertexdata`]), into a [`Processed`].
//!   A Mesh's `CompressedMesh` is unpacked (`PackedBitVector`'s
//!   `unpack_ints` and `unpack_floats`, then `decompress_compressed_mesh`)
//!   with UnityPy's two Tier C errors there corrected (`compressed`'s
//!   module docs).
//! - [`Processed::get_triangles`]: each submesh's triangles, for triangle
//!   lists, strips and quads, with UnityPy's errors for the rest.
//!
//! **What is not here** (`docs/plans/extract.md` E16). A Mesh's own vertex
//! arrays and a sprite's own `vertices`, which no version's typetree holds
//! beside `m_VertexData`, are P2a's rest; below Unity 4, `get_channels`'
//! fixed layout is P4. Each is [`MeshError::Unsupported`], returned where
//! UnityPy would begin it, so every failure before it is UnityPy's.
//!
//! **Divergences** (`PARITY.md` sections 15.3, 19.1 and 19.3): a compressed
//! mesh's third and later UV channels read from where the ones before end,
//! and a skin's fourth weight in 31sts, where UnityPy errs (Tier C); a
//! packed item wider than 32 bits is [`MeshError::Malformed`]; an array or a
//! loop past the caller's meter is [`MeshError::Charge`]; a gather whose
//! 32-bit offsets wrap past UnityPyBoost's check, where UnityPy reads or
//! writes out of bounds, is [`MeshError::WrappedAccess`]; a field of a type
//! or range no version's typetree gives it, or a class node of another
//! type, is [`MeshError::Malformed`]; and the core's resource read reports
//! its own errors ([`MeshError::Resource`]). The handler changes nothing it
//! reads: UnityPy's `process` sets a streamed Mesh's `m_DataSize` and leaves
//! its results on the handler for a second call to read, and its
//! `get_triangles` rewrites a strip's `indexCount`, which
//! [`SubMeshTriangles::index_count`] returns instead; so each call here
//! gives the first's answer.
//!
//! **No panics, bounded work** (`docs/plans/core.md` section 3.15): indexing,
//! `unwrap` and the like are denied outside tests; offsets are computed as
//! UnityPyBoost computes them, in wrapping `u32`s, and checked. Every array
//! sized by the input (the values, the index buffer, the triangles, the
//! derived streams) is charged to the caller's meter, at its element count,
//! before it is allocated, and a message echoes at most 64 bytes of a type
//! name. Every loop's work is counted on the meter before the loop runs, and
//! after the charge that loop's output needs, so a refused charge counts no
//! work: a unit per byte or element of output, per channel (those skipped
//! included), per submesh, and for every name a scan of a class's fields may
//! compare, counted before it, so the work is in proportion to the input or
//! the output.
//! `tests/oracle.rs` replays what the oracle recorded
//! (`tools/fixtures/meshdecode.py`); `tests/limits.rs` runs each recorded
//! case under smaller meters and work bounds, and with its bytes changed;
//! `tests/charges.rs` counts the allocations each case makes against its
//! charges; and `tests/work.rs` measures the work against the channels,
//! submeshes and fields read.

#![forbid(unsafe_code)]
// No panics on a mesh's contents (plan section 3.15). Tests may opt out.
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

mod compressed;
mod error;
mod format;
mod handler;
mod packed;
mod triangles;
mod unpack;
mod version;
mod vertex;
mod view;

pub use error::{IndexKind, MeshError, StructError, Unsupported};
pub use format::{
    FormatTable, GfxPrimitiveType, MeshTopology, StructCode, VertexChannelFormat, VertexFormat,
    VertexFormat2017, get_channel_component_size, get_channel_dtype,
};
pub use handler::{FileResources, IndexBuffer, MeshHandler, Processed, Resources, Source};
pub use triangles::{SubMeshTriangles, Triangles};
pub use unpack::{Component, ComponentBytes, Gather, Values, VertexArray, unpack_vertexdata};
pub use version::Version;

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16).
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Plan 3.16: every public type is `Send + Sync`, so the engine's pool
    /// threads may move and share them.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }

    /// Plan 4.1: no thread, no wait; the handler runs on the engine's
    /// threads.
    #[test]
    fn the_handler_starts_no_thread_and_never_waits() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "waits or starts a thread: {found:?}");
    }
}
