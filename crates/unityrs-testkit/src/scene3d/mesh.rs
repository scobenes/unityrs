// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py's Mesh, BlendShapeData and MeshLodInfo field definitions. See THIRD_PARTY_LICENSES.
//! Hand-built Mesh writers using the testkit's typed values. These are synthetic
//! layouts, from the pinned MIT class definitions and meshdecode's existing
//! synthetic fixtures; no generated typetrees or input data are used.
use crate::ngui::{Object, V, st, vector};

/// The modern Mesh layouts this builder exercises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshVersion {
    /// Unity 2019.4.
    V2019,
    /// Unity 2022.3.
    V2022,
    /// Unity 6000.3, with mesh LOD metadata.
    V6000,
}
impl MeshVersion {
    /// The serialized file's Unity version.
    #[must_use]
    pub fn version(self) -> &'static str {
        match self {
            Self::V2019 => "2019.4.0f1",
            Self::V2022 => "2022.3.0f1",
            Self::V6000 => "6000.3.0f1",
        }
    }
}
/// A Vector3f of the given values.
#[must_use]
pub fn vec3(v: [f32; 3]) -> V {
    st(
        "Vector3f",
        vec![
            ("x", V::Float(v[0])),
            ("y", V::Float(v[1])),
            ("z", V::Float(v[2])),
        ],
    )
}
/// Axis-aligned bounds.
#[must_use]
pub fn aabb(center: [f32; 3], extent: [f32; 3]) -> V {
    st(
        "AABB",
        vec![("m_Center", vec3(center)), ("m_Extent", vec3(extent))],
    )
}
/// A Matrix4x4f in row-major order.
#[must_use]
pub fn matrix(values: [f32; 16]) -> V {
    let names = [
        "e00", "e01", "e02", "e03", "e10", "e11", "e12", "e13", "e20", "e21", "e22", "e23", "e30",
        "e31", "e32", "e33",
    ];
    V::Struct(
        "Matrix4x4f".to_owned(),
        names
            .into_iter()
            .zip(values)
            .map(|(n, v)| (n.to_owned(), V::Float(v)))
            .collect(),
    )
}
/// A sparse blend-shape vertex.
#[must_use]
pub fn blend_vertex(index: u32, vertex: [f32; 3], normal: [f32; 3], tangent: [f32; 3]) -> V {
    st(
        "BlendShapeVertex",
        vec![
            ("vertex", vec3(vertex)),
            ("normal", vec3(normal)),
            ("tangent", vec3(tangent)),
            ("index", V::UInt(index)),
        ],
    )
}
/// A frame's sparse-vertex range and attribute flags.
#[must_use]
pub fn blend_frame(first: u32, count: u32, normals: bool, tangents: bool) -> V {
    st(
        "MeshBlendShape",
        vec![
            ("firstVertex", V::UInt(first)),
            ("vertexCount", V::UInt(count)),
            ("hasNormals", V::Bool(normals)),
            ("hasTangents", V::Bool(tangents)),
        ],
    )
}
/// A named channel's range of frames.
#[must_use]
pub fn blend_channel(name: &[u8], hash: u32, first: i32, count: i32) -> V {
    st(
        "MeshBlendShapeChannel",
        vec![
            ("name", V::Str(name.to_vec())),
            ("nameHash", V::UInt(hash)),
            ("frameIndex", V::Int(first)),
            ("frameCount", V::Int(count)),
        ],
    )
}
/// Complete `BlendShapeData`. Empty arrays retain their element shapes.
#[must_use]
pub fn blend_shapes(vertices: Vec<V>, frames: Vec<V>, channels: Vec<V>, weights: Vec<f32>) -> V {
    // Frames can include legacy fields; use the first frame's shape when present.
    let frame_shape = frames
        .first()
        .cloned()
        .unwrap_or_else(|| blend_frame(0, 0, false, false));
    st(
        "BlendShapeData",
        vec![
            (
                "vertices",
                vector(blend_vertex(0, [0.0; 3], [0.0; 3], [0.0; 3]), vertices),
            ),
            ("shapes", vector(frame_shape, frames)),
            ("channels", vector(blend_channel(b"", 0, 0, 0), channels)),
            (
                "fullWeights",
                vector(V::Float(0.0), weights.into_iter().map(V::Float).collect()),
            ),
        ],
    )
}
/// Mesh LOD fields from the pinned class definitions. Ranges are (start, count).
#[must_use]
pub fn lod_info(levels: u32, bias: f32, slope: f32, subs: &[Vec<(u32, u32)>]) -> V {
    let range = |start, count| {
        st(
            "MeshLodRange",
            vec![
                ("m_IndexStart", V::UInt(start)),
                ("m_IndexCount", V::UInt(count)),
            ],
        )
    };
    let sub = |ranges: &[(u32, u32)]| {
        st(
            "MeshLodSubMesh",
            vec![(
                "m_Levels",
                vector(
                    range(0, 0),
                    ranges.iter().map(|&(s, c)| range(s, c)).collect(),
                ),
            )],
        )
    };
    st(
        "MeshLodInfo",
        vec![
            ("m_NumLevels", V::UInt(levels)),
            (
                "m_SubMeshes",
                vector(sub(&[]), subs.iter().map(|v| sub(v)).collect()),
            ),
            (
                "m_LodSelectionCurve",
                st(
                    "LodSelectionCurve",
                    vec![
                        ("m_LodBias", V::Float(bias)),
                        ("m_LodSlope", V::Float(slope)),
                    ],
                ),
            ),
        ],
    )
}
/// One submesh. firstByte and indexCount refer to the source index buffer.
#[must_use]
pub fn submesh(
    first_byte: u32,
    index_count: u32,
    topology: i32,
    base_vertex: u32,
    first_vertex: u32,
    vertex_count: u32,
) -> V {
    st(
        "SubMesh",
        vec![
            ("firstByte", V::UInt(first_byte)),
            ("indexCount", V::UInt(index_count)),
            ("topology", V::Int(topology)),
            ("baseVertex", V::UInt(base_vertex)),
            ("firstVertex", V::UInt(first_vertex)),
            ("vertexCount", V::UInt(vertex_count)),
            ("localAABB", aabb([0.0; 3], [1.0; 3])),
        ],
    )
}
fn packed(float: bool) -> V {
    let mut fields = vec![("m_NumItems", V::UInt(0))];
    if float {
        fields.extend([("m_Range", V::Float(0.0)), ("m_Start", V::Float(0.0))]);
    }
    fields.extend([
        ("m_Data", vector(V::UInt8(0), vec![])),
        ("m_BitSize", V::UInt8(0)),
    ]);
    st("PackedBitVector", fields)
}
/// An empty compressed mesh in the modern layout.
#[must_use]
pub fn compressed() -> V {
    st(
        "CompressedMesh",
        vec![
            ("m_Vertices", packed(true)),
            ("m_UV", packed(true)),
            ("m_Normals", packed(true)),
            ("m_Tangents", packed(true)),
            ("m_Weights", packed(false)),
            ("m_NormalSigns", packed(false)),
            ("m_TangentSigns", packed(false)),
            ("m_FloatColors", packed(true)),
            ("m_BoneIndices", packed(false)),
            ("m_Triangles", packed(false)),
            ("m_UVInfo", V::UInt(0)),
        ],
    )
}
/// A position-only Mesh with a 32-bit index buffer and one triangle-list
/// submesh. Callers replace metadata through `ngui::set_field` for richer cases.
#[must_use]
pub fn mesh(
    path_id: i64,
    name: &[u8],
    vertices: &[[f32; 3]],
    indices: &[u32],
    version: MeshVersion,
) -> Object {
    let mut data: Vec<u8> = vertices
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    data.resize(data.len().next_multiple_of(16), 0);
    let channel = |dimension| {
        st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(0)),
                ("format", V::UInt8(0)),
                ("dimension", V::UInt8(dimension)),
            ],
        )
    };
    let mut channels = vec![channel(3)];
    channels.extend((1..14).map(|_| channel(0)));
    let count = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let mut fields = vec![
        ("m_Name", V::Str(name.to_vec())),
        (
            "m_SubMeshes",
            vector(
                submesh(0, 0, 0, 0, 0, 0),
                vec![submesh(
                    0,
                    u32::try_from(indices.len()).unwrap_or(u32::MAX),
                    0,
                    0,
                    0,
                    count,
                )],
            ),
        ),
        ("m_Shapes", blend_shapes(vec![], vec![], vec![], vec![])),
        ("m_BindPose", vector(matrix([0.0; 16]), vec![])),
        ("m_BoneNameHashes", vector(V::UInt(0), vec![])),
        ("m_RootBoneNameHash", V::UInt(0)),
        ("m_MeshCompression", V::UInt8(0)),
        ("m_IndexFormat", V::Int(1)),
        (
            "m_IndexBuffer",
            vector(
                V::UInt8(0),
                indices
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .map(V::UInt8)
                    .collect(),
            ),
        ),
        (
            "m_VertexData",
            st(
                "VertexData",
                vec![
                    ("m_VertexCount", V::UInt(count)),
                    ("m_Channels", vector(channel(0), channels)),
                    ("m_DataSize", V::Bytes(data)),
                ],
            ),
        ),
        ("m_CompressedMesh", compressed()),
        ("m_LocalAABB", aabb([0.0; 3], [1.0; 3])),
        ("m_MeshUsageFlags", V::Int(0)),
    ];
    if version == MeshVersion::V6000 {
        fields.push(("m_MeshLodInfo", lod_info(0, 0.0, 0.0, &[])));
    }
    Object::new(path_id, 43, st("Mesh", fields))
}
