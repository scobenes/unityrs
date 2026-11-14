//! Owned handler arrays, with their original Python numeric types and charges.
use crate::handler::IndexBuffer;
use crate::unpack::VertexArray;
use crate::vertex::Attributes;

/// A processed mesh independent of its parsed source. Each buffer retains its charge.
#[derive(Debug)]
pub struct ProcessedOwned {
    pub(crate) vertex_count: u64,
    pub(crate) use_16bit: bool,
    pub(crate) index_buffer: Option<IndexBuffer>,
    pub(crate) attrs: Attributes,
}
const _: () = crate::send_sync::<ProcessedOwned>();
impl ProcessedOwned {
    /// `m_VertexCount`.
    #[must_use]
    pub fn vertex_count(&self) -> u64 {
        self.vertex_count
    }

    /// `m_Use16BitIndices`.
    #[must_use]
    pub fn use_16bit_indices(&self) -> bool {
        self.use_16bit
    }

    /// `m_IndexBuffer`: `None` where the source had none.
    #[must_use]
    pub fn index_buffer(&self) -> Option<&IndexBuffer> {
        self.index_buffer.as_ref()
    }

    /// `m_Vertices`.
    #[must_use]
    pub fn vertices(&self) -> Option<&VertexArray> {
        self.attrs.vertices.as_ref()
    }

    /// `m_Normals`.
    #[must_use]
    pub fn normals(&self) -> Option<&VertexArray> {
        self.attrs.normals.as_ref()
    }

    /// `m_Tangents`.
    #[must_use]
    pub fn tangents(&self) -> Option<&VertexArray> {
        self.attrs.tangents.as_ref()
    }

    /// `m_Colors`.
    #[must_use]
    pub fn colors(&self) -> Option<&VertexArray> {
        self.attrs.colors.as_ref()
    }

    /// `m_UV0` to `m_UV7`: `uv(i)`, `None` past 7.
    #[must_use]
    pub fn uv(&self, i: usize) -> Option<&VertexArray> {
        self.attrs.uv.get(i).and_then(Option::as_ref)
    }

    /// `m_BoneWeights`.
    #[must_use]
    pub fn bone_weights(&self) -> Option<&VertexArray> {
        self.attrs.bone_weights.as_ref()
    }

    /// `m_BoneIndices`.
    #[must_use]
    pub fn bone_indices(&self) -> Option<&VertexArray> {
        self.attrs.bone_indices.as_ref()
    }

    /// `m_BindPose`, which a compressed mesh below 5 sets: each matrix's
    /// 16 `float`s flat, row by row, so a tuple of 16 (UnityPy's
    /// `reshape(shape=(4, 4))` gives each as four rows of 4, the last
    /// matrix's rows cut short, then empty, where the count is no multiple
    /// of 16).
    #[must_use]
    pub fn bind_pose(&self) -> Option<&VertexArray> {
        self.attrs.bind_pose.as_ref()
    }
}
