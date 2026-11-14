// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py's Mesh, BlendShapeData and MeshLodInfo fields. See THIRD_PARTY_LICENSES.
//! Each variable-length field owns its charge, including nested LOD ranges and names.
use crate::{ProcessedOwned, Triangles};
use unityrs_core::limits::MeteredVec;

/// An owned raw Unity string, including bytes decoded with surrogateescape.
#[derive(Debug)]
pub struct RawString(pub(crate) MeteredVec<u8>);
const _: () = crate::send_sync::<RawString>();
impl RawString {
    /// Original bytes, without UTF-8 replacement.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// A Mesh's geometry and metadata. All coordinates and weights are as read.
#[derive(Debug)]
pub struct Mesh {
    /// `m_Name`.
    pub name: RawString,
    /// `MeshHandler` arrays, with their Python int/float tags.
    pub processed: ProcessedOwned,
    /// Source submesh records; strips' source counts are preserved.
    pub submeshes: MeteredVec<SubMesh>,
    /// Handler triangles, including its cut and empty tuples and updated strip counts.
    pub triangles: Triangles,
    /// `m_LocalAABB`, if present.
    pub bounds: Option<Aabb>,
    /// Flat row-major bind matrices; below Unity 5, compressed matrices take precedence.
    /// Each `chunks_exact(16)` item is a complete matrix; its remainder is the
    /// trailing partial matrix UnityPy retains (possibly with partial/empty rows).
    /// The scalar count is preserved: no padding, truncation or invented values.
    /// Collectors can consume complete matrices and handle the remainder by policy.
    pub bind_poses: MeteredVec<f32>,
    /// `m_BoneNameHashes`.
    pub bone_name_hashes: MeteredVec<u32>,
    /// `m_RootBoneNameHash`, if present.
    pub root_bone_name_hash: Option<u32>,
    /// All sparse deltas, frames, channels and full weights, without choosing a frame.
    pub blend_shapes: BlendShapes,
    /// Unity 6 mesh LOD information, if present.
    pub lod: Option<MeshLods>,
}
const _: () = crate::send_sync::<Mesh>();

/// Axis-aligned bounds as stored by Unity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// `m_Center`.
    pub center: [f32; 3],
    /// `m_Extent` (half-size).
    pub extent: [f32; 3],
}
const _: () = crate::send_sync::<Aabb>();

/// A source submesh record. Optional fields distinguish old layouts from zeroes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubMesh {
    /// Byte offset into the source index buffer.
    pub first_byte: u32,
    /// Source `indexCount`, unchanged by triangle-strip expansion.
    pub index_count: u32,
    /// Source topology; absent on some legacy layouts.
    pub topology: Option<i32>,
    /// Base vertex, if stored. The handler's triangles do not apply this offset.
    pub base_vertex: Option<u32>,
    /// First vertex in the submesh.
    pub first_vertex: u32,
    /// Vertex count.
    pub vertex_count: u32,
    /// `localAABB`, if present.
    pub bounds: Option<Aabb>,
}
const _: () = crate::send_sync::<SubMesh>();

/// Sparse blend-shape data with the source arrays and ordering preserved.
#[derive(Debug)]
pub struct BlendShapes {
    /// `vertices`.
    pub vertices: MeteredVec<BlendShapeVertex>,
    /// `shapes`, one record per frame.
    pub frames: MeteredVec<BlendShapeFrame>,
    /// `channels`, including multi-frame channels.
    pub channels: MeteredVec<BlendShapeChannel>,
    /// `fullWeights`, one source weight per frame.
    pub full_weights: MeteredVec<f32>,
}
const _: () = crate::send_sync::<BlendShapes>();

/// A sparse vertex delta, indexed into the mesh's vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendShapeVertex {
    /// Source vertex index.
    pub index: u32,
    /// Position delta.
    pub vertex: [f32; 3],
    /// Normal delta.
    pub normal: [f32; 3],
    /// Tangent delta.
    pub tangent: [f32; 3],
}
const _: () = crate::send_sync::<BlendShapeVertex>();

/// One frame's range into the sparse delta array, and the legacy optional fields.
#[derive(Debug)]
pub struct BlendShapeFrame {
    /// First sparse delta.
    pub first_vertex: u32,
    /// Number of sparse deltas.
    pub vertex_count: u32,
    /// Normals are available for this frame.
    pub has_normals: bool,
    /// Tangents are available for this frame.
    pub has_tangents: bool,
    /// Legacy per-frame name, when present.
    pub name: Option<RawString>,
    /// Legacy delta bounds, when present.
    pub aabb_min_delta: Option<[f32; 3]>,
    /// Legacy delta bounds, when present.
    pub aabb_max_delta: Option<[f32; 3]>,
}
const _: () = crate::send_sync::<BlendShapeFrame>();

/// A named blend-shape channel's range into the frame array.
#[derive(Debug)]
pub struct BlendShapeChannel {
    /// Original name bytes.
    pub name: RawString,
    /// Source name hash.
    pub name_hash: u32,
    /// First frame; kept signed as stored, including malformed negative ranges.
    pub frame_index: i32,
    /// Number of frames, kept signed as stored.
    pub frame_count: i32,
}
const _: () = crate::send_sync::<BlendShapeChannel>();

/// Mesh LOD metadata. This model does not choose or validate a scene LOD.
#[derive(Debug)]
pub struct MeshLods {
    /// `m_NumLevels`.
    pub num_levels: u32,
    /// `m_LodSelectionCurve.m_LodBias`.
    pub lod_bias: f32,
    /// `m_LodSelectionCurve.m_LodSlope`.
    pub lod_slope: f32,
    /// `m_SubMeshes` in source order.
    pub submeshes: MeteredVec<MeshLodSubMesh>,
}
const _: () = crate::send_sync::<MeshLods>();

/// The index ranges of each LOD for one submesh.
#[derive(Debug)]
pub struct MeshLodSubMesh {
    /// `m_Levels` in source order, without padding to `m_NumLevels`.
    pub levels: MeteredVec<MeshLodRange>,
}
const _: () = crate::send_sync::<MeshLodSubMesh>();

/// One LOD's range of source indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshLodRange {
    /// `m_IndexStart`.
    pub index_start: u32,
    /// `m_IndexCount`.
    pub index_count: u32,
}
const _: () = crate::send_sync::<MeshLodRange>();
