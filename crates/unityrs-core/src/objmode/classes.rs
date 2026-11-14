// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py, object-mode class fields. See THIRD_PARTY_LICENSES.
/// A class's attrs fields (`classes/generated.py`), and whether object
/// mode builds it as an `UnknownObject` when given a field it lacks
/// (`PARITY.md` section 9.2.1: a non-empty `__slots__`).
#[derive(Debug)]
pub struct Class {
    /// The node types that look this class up (a node's `m_Type`).
    pub types: &'static [&'static str],
    /// Fields without a default.
    pub required: &'static [&'static str],
    /// Fields whose default is `None`.
    pub optional: &'static [&'static str],
    /// A non-empty `__slots__`: an extra field gives an `UnknownObject`.
    pub slotted: bool,
}

/// `Mesh` (`classes/generated.py:4605-4642`), whose `__slots__` is `()`.
pub const MESH: Class = Class {
    types: &["Mesh"],
    required: &[
        "m_BindPose",
        "m_CompressedMesh",
        "m_IndexBuffer",
        "m_LocalAABB",
        "m_MeshCompression",
        "m_MeshUsageFlags",
        "m_Name",
        "m_SubMeshes",
    ],
    optional: &[
        "m_BakedConvexCollisionMesh",
        "m_BakedTriangleCollisionMesh",
        "m_BoneNameHashes",
        "m_BonesAABB",
        "m_CollisionTriangles",
        "m_CollisionVertexCount",
        "m_Colors",
        "m_CookingOptions",
        "m_IndexFormat",
        "m_IsReadable",
        "m_KeepIndices",
        "m_KeepVertices",
        "m_MeshLodInfo",
        "m_MeshMetrics_0_",
        "m_MeshMetrics_1_",
        "m_Normals",
        "m_RootBoneNameHash",
        "m_ShapeVertices",
        "m_Shapes",
        "m_Skin",
        "m_StreamCompression",
        "m_StreamData",
        "m_Tangents",
        "m_UV",
        "m_UV1",
        "m_Use16BitIndices",
        "m_VariableBoneCountWeights",
        "m_VertexData",
        "m_Vertices",
    ],
    slotted: false,
};

/// `Sprite` (`classes/generated.py:4867-4882`), whose `__slots__` is `()`.
pub const SPRITE: Class = Class {
    types: &["Sprite"],
    required: &[
        "m_Extrude",
        "m_Name",
        "m_Offset",
        "m_PixelsToUnits",
        "m_RD",
        "m_Rect",
    ],
    optional: &[
        "m_AtlasTags",
        "m_Bones",
        "m_Border",
        "m_IsPolygon",
        "m_PhysicsShape",
        "m_Pivot",
        "m_RenderDataKey",
        "m_ScriptableObjects",
        "m_SpriteAtlas",
    ],
    slotted: false,
};

/// `SpriteRenderData` (`classes/generated.py:10903-10919`).
pub const SPRITE_RENDER_DATA: Class = Class {
    types: &["SpriteRenderData"],
    required: &["settingsRaw", "texture", "textureRect", "textureRectOffset"],
    optional: &[
        "alphaTexture",
        "atlasRectOffset",
        "downscaleMultiplier",
        "indices",
        "m_Bindpose",
        "m_IndexBuffer",
        "m_SourceSkin",
        "m_SubMeshes",
        "m_VertexData",
        "secondaryTextures",
        "uvTransform",
        "vertices",
    ],
    slotted: true,
};

/// `VertexData` (`classes/generated.py:11800-11809`).
pub const VERTEX_DATA: Class = Class {
    types: &["VertexData"],
    required: &["m_DataSize", "m_VertexCount"],
    optional: &[
        "m_Channels",
        "m_CurrentChannels",
        "m_Streams",
        "m_Streams_0_",
        "m_Streams_1_",
        "m_Streams_2_",
        "m_Streams_3_",
    ],
    slotted: true,
};

/// `ChannelInfo` (`classes/generated.py:7213-7217`).
pub const CHANNEL_INFO: Class = Class {
    types: &["ChannelInfo"],
    required: &["dimension", "format", "offset", "stream"],
    optional: &[],
    slotted: true,
};

/// `StreamInfo` (`classes/generated.py:11043-11049`).
pub const STREAM_INFO: Class = Class {
    types: &["StreamInfo"],
    required: &["channelMask", "offset", "stride"],
    optional: &["align", "dividerOp", "frequency"],
    slotted: true,
};

/// `SubMesh` (`classes/generated.py:11098-11107`).
pub const SUB_MESH: Class = Class {
    types: &["SubMesh"],
    required: &[
        "firstByte",
        "firstVertex",
        "indexCount",
        "localAABB",
        "vertexCount",
    ],
    optional: &["baseVertex", "isTriStrip", "topology", "triangleCount"],
    slotted: true,
};

/// `StreamingInfo` (`classes/generated.py:11067-11070`).
pub const STREAMING_INFO: Class = Class {
    types: &["StreamingInfo"],
    required: &["offset", "path", "size"],
    optional: &[],
    slotted: true,
};

/// `CompressedMesh` (`classes/generated.py:7488-7501`).
pub const COMPRESSED_MESH: Class = Class {
    types: &["CompressedMesh"],
    required: &[
        "m_BoneIndices",
        "m_NormalSigns",
        "m_Normals",
        "m_TangentSigns",
        "m_Tangents",
        "m_Triangles",
        "m_UV",
        "m_Vertices",
        "m_Weights",
    ],
    optional: &["m_BindPoses", "m_Colors", "m_FloatColors", "m_UVInfo"],
    slotted: true,
};

/// `PackedBitVector` (`classes/generated.py:9160-9165`).
pub const PACKED_BIT_VECTOR: Class = Class {
    types: &["PackedBitVector"],
    required: &["m_Data", "m_NumItems"],
    optional: &["m_BitSize", "m_Range", "m_Start"],
    slotted: true,
};

/// A skin element: `BoneWeights4` or `BoneInfluence`, the same fields
/// (`classes/generated.py:6956-6976`); `m_Skin` is a list of either.
pub const BONE_WEIGHTS: Class = Class {
    types: &["BoneWeights4", "BoneInfluence"],
    required: &[
        "boneIndex_0_",
        "boneIndex_1_",
        "boneIndex_2_",
        "boneIndex_3_",
        "weight_0_",
        "weight_1_",
        "weight_2_",
        "weight_3_",
    ],
    optional: &[],
    slotted: true,
};

const _: () = crate::send_sync::<Class>();
