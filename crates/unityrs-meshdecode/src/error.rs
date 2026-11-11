// Ported from UnityPy 1.25.2 (MIT): the exceptions UnityPy/helpers/MeshHelper.py's MeshHandler and UnityPyBoost/Mesh.cpp's unpack_vertexdata raise. See THIRD_PARTY_LICENSES.
//! What [`crate::MeshHandler::process`] and
//! [`crate::Processed::get_triangles`] return when they fail.
//!
//! Each variant but the last five is an exception UnityPy raises, and
//! [`MeshError::python`] gives its class name and message as UnityPy gives
//! them (`type(e).__name__`, `str(e)`), matching `CPython` 3.11's messages for
//! the struct, list and str errors they reach, which the oracle's fixtures
//! record
//! and `tests/oracle.rs` compares. The last five are where the crate stops
//! and UnityPy does not, each a divergence `PARITY.md` states (section 15.3,
//! section 19.3): a work or memory bound ([`MeshError::Charge`]), an access
//! UnityPyBoost makes out of bounds after its `uint32_t` arithmetic wraps
//! ([`MeshError::WrappedAccess`]), a part of `MeshHandler` that a later
//! phase ports ([`MeshError::Unsupported`]), a value of a type or range the
//! class's typetree never declares ([`MeshError::Malformed`]), and the
//! core's own resource error ([`MeshError::Resource`]), which is the
//! core's port of `get_resource_data`'s failures.

use std::fmt;

use unityrs_core::limits::ChargeError;

use crate::format::FormatTable;

/// A failure of the mesh handler (module docs).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MeshError {
    /// `AssertionError`: one of `process`'s or `get_triangles`'s asserts
    /// (`helpers/MeshHelper.py:113`, `:119-124`, `:134`, `:138`, `:148`,
    /// `:631-632`). The message is the assert's own, empty for a bare one.
    #[error("{0}")]
    Assertion(&'static str),
    /// `AttributeError`: a field read on an `UnknownObject`, the class
    /// UnityPyBoost's object mode builds when the class named by the
    /// node's type cannot take its fields (`PARITY.md` section 9.2), that
    /// lacks it (`'UnknownObject' object has no attribute 'm_Streams'`).
    #[error("'UnknownObject' object has no attribute '{0}'")]
    NoAttribute(&'static str),
    /// `ValueError`: the source is neither a `Mesh` nor a
    /// `SpriteRenderData`, because object mode built it as an
    /// `UnknownObject` (`helpers/MeshHelper.py:171-172`).
    #[error("Unknown mesh type <class 'UnityPy.classes.UnknownObject.UnknownObject'>")]
    UnknownMeshType,
    /// `ValueError`: the version's type byte is no `UnityVersionType`, which
    /// its first index raises (`helpers/UnityVersion.py:41-42`, `:99-107`).
    #[error("{0} is not a valid UnityVersionType")]
    VersionType(u8),
    /// `ValueError`: a channel's format is not a member of the version's
    /// format enum (`helpers/MeshHelper.py:451-462`).
    #[error("{format} is not a valid {}", table.python_name())]
    InvalidFormat {
        /// The channel's `format`.
        format: u8,
        /// The enum the version picks.
        table: FormatTable,
    },
    /// `ValueError`: a channel past the version's last
    /// (`helpers/MeshHelper.py:395-449`).
    #[error("Unknown channel {0}")]
    UnknownChannel(u64),
    /// `ValueError`: `get_streams`'s `max` over no channels
    /// (`helpers/MeshHelper.py:257`).
    #[error("max() arg is an empty sequence")]
    NoChannels,
    /// `ValueError`: UnityPyBoost's bounds check
    /// (`UnityPyBoost/Mesh.cpp:50-56`).
    #[error("Vertex data access out of bounds")]
    OutOfBounds,
    /// `struct.error` (its class's name is `error`).
    #[error("{0}")]
    Struct(StructError),
    /// `IndexError`.
    #[error("{0}")]
    Index(IndexKind),
    /// `ValueError`: a strip's or a quad's indices unpacked from a slice
    /// the index buffer cuts short (`helpers/MeshHelper.py:655`, `:673`).
    #[error("not enough values to unpack (expected {expected}, got {got})")]
    Unpack {
        /// 3 for a strip, 4 for a quad.
        expected: u8,
        /// What the slice held.
        got: u8,
    },
    /// `ValueError`: `bytes()` of an index list holding a value past a
    /// byte (`helpers/MeshHelper.py:175`).
    #[error("bytes must be in range(0, 256)")]
    ByteRange,
    /// `ValueError`: a submesh of lines, points or another topology
    /// (`helpers/MeshHelper.py:678-679`).
    #[error("Failed getting triangles. Submesh topology is lines or points.")]
    Topology,
    /// The core's error reading `m_StreamData`'s bytes (`Environment::resource`,
    /// its port of `get_resource_data`), as the core reports it.
    #[error("resource: {0}")]
    Resource(unityrs_core::Error),
    /// A charge the caller's meter refused: an array's memory, or the
    /// work counted before a loop (`docs/plans/extract.md` E59). UnityPy
    /// has no such bound (`PARITY.md` section 19.3).
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// UnityPyBoost's `uint32_t` arithmetic wrapped and its check passed,
    /// so it would read outside the vertex data or write past the buffer
    /// it allocated, which is undefined behaviour in UnityPy
    /// (`UnityPyBoost/Mesh.cpp:47-75`); the crate refuses before reading
    /// (`PARITY.md` section 19.3).
    #[error("vertex data access wraps the 32-bit offsets UnityPyBoost computes")]
    WrappedAccess,
    /// A part of `MeshHandler` a later phase ports (module docs of
    /// [`crate::Unsupported`]).
    #[error("unsupported: {0}")]
    Unsupported(Unsupported),
    /// A field whose value has a type or a range no version's typetree
    /// gives it, or a node whose type is not the class the field's
    /// typetree names, which UnityPy's arithmetic or object mode would take
    /// in ways no test characterises (`PARITY.md` section 15.3, Tier U).
    #[error("malformed mesh: {0}")]
    Malformed(String),
}

const _: () = crate::send_sync::<MeshError>();

impl MeshError {
    /// The exception UnityPy raises for this failure: its class's
    /// `__name__` and `str(e)`. `None` for the crate's own refusals (the
    /// last five variants, module docs).
    #[must_use]
    pub fn python(&self) -> Option<(&'static str, String)> {
        let class = match self {
            Self::Assertion(_) => "AssertionError",
            Self::NoAttribute(_) => "AttributeError",
            Self::UnknownMeshType
            | Self::VersionType(_)
            | Self::InvalidFormat { .. }
            | Self::UnknownChannel(_)
            | Self::NoChannels
            | Self::OutOfBounds
            | Self::Unpack { .. }
            | Self::ByteRange
            | Self::Topology => "ValueError",
            Self::Struct(_) => "error",
            Self::Index(_) => "IndexError",
            Self::Resource(_)
            | Self::Charge(_)
            | Self::WrappedAccess
            | Self::Unsupported(_)
            | Self::Malformed(_) => return None,
        };
        Some((class, self.to_string()))
    }

    /// A [`MeshError::Malformed`] naming what was wrong.
    pub(crate) fn malformed(detail: impl Into<String>) -> Self {
        Self::Malformed(detail.into())
    }
}

/// A `struct.error` Python 3.11 raises, by its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructError {
    /// `struct.iter_unpack` of a format of zero bytes (a channel whose
    /// dimension's low four bits are 0, `helpers/MeshHelper.py:392`):
    /// "cannot iteratively unpack with a struct of length 0".
    ZeroLength,
    /// `struct.unpack` of an index buffer whose length is not a multiple
    /// of the index size (`helpers/MeshHelper.py:183-186`): "unpack
    /// requires a buffer of N bytes", N the bytes the count asks for.
    UnpackSize(u64),
}

const _: () = crate::send_sync::<StructError>();

impl fmt::Display for StructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLength => f.write_str("cannot iteratively unpack with a struct of length 0"),
            Self::UnpackSize(n) => write!(f, "unpack requires a buffer of {n} bytes"),
        }
    }
}

/// An `IndexError`, by its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexKind {
    /// "list index out of range": a 4.x channel's `stream` past the
    /// file's streams (`helpers/MeshHelper.py:341`).
    List,
    /// "string index out of range": a 4.x channel's index past its
    /// stream's `bin(channelMask)` (`helpers/MeshHelper.py:346-347`).
    Str,
    /// "list assignment index out of range": a quad submesh whose index
    /// count is not a multiple of 4 (`helpers/MeshHelper.py:674-675`).
    ListAssignment,
}

const _: () = crate::send_sync::<IndexKind>();

impl fmt::Display for IndexKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::List => "list index out of range",
            Self::Str => "string index out of range",
            Self::ListAssignment => "list assignment index out of range",
        })
    }
}

/// A part of `MeshHandler` this crate leaves to a later phase
/// (`docs/plans/extract.md` E16, `PARITY.md` section 15.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unsupported {
    /// A version below 4: `get_channels`' fixed layout
    /// (`helpers/MeshHelper.py:283-321`), P4.
    FixedChannels,
    /// A `CompressedMesh` with data `decompress_compressed_mesh` would
    /// unpack (`helpers/MeshHelper.py:468-628`), with `PackedBitVector`
    /// and `normalize`: P2a. The field named is the first that holds any.
    CompressedMesh(&'static str),
    /// A Mesh's own vertex arrays (`m_Vertices`, `m_Normals`,
    /// `m_Tangents`, `m_UV`, `m_UV1`, `m_Colors`), which `copy_from_mesh`
    /// copies (`helpers/MeshHelper.py:206-224`): a Mesh has them only below
    /// 3.5 (as AssetStudio v0.16.47's MIT `Classes/Mesh.cs` reads them),
    /// where `process` has already failed its assert, so only a tree no
    /// Unity version writes holds them beside `m_VertexData`. P2a, with the
    /// rest of `copy_from_mesh`'s legacy part.
    MeshArray(&'static str),
    /// A sprite's own `vertices` (`helpers/MeshHelper.py:246-251`): a
    /// render data has them only below 5.6 (AssetStudio v0.16.47's
    /// `Classes/Sprite.cs`), which has no `m_VertexData`, so the same holds.
    /// P2a.
    SpriteVertices,
}

const _: () = crate::send_sync::<Unsupported>();

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FixedChannels => f.write_str("the fixed channels of a version below 4 (P4)"),
            Self::CompressedMesh(field) => {
                write!(f, "a CompressedMesh whose {field} holds data (P2a)")
            }
            Self::MeshArray(field) => write!(f, "a Mesh's own {field} array (P2a)"),
            Self::SpriteVertices => f.write_str("a sprite's own vertices (P2a)"),
        }
    }
}
