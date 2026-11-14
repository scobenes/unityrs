//! Blend-shape and mesh LOD metadata, preserving the source array ranges.
use crate::fields::{Fields, float};
use crate::{
    BlendShapeChannel, BlendShapeFrame, BlendShapeVertex, BlendShapes, MeshError, MeshLodRange,
    MeshLodSubMesh, MeshLods,
};
use unityrs_core::limits::MeteredVec;

pub(crate) fn read(root: Fields<'_>) -> Result<BlendShapes, MeshError> {
    // Before BlendShapeData, Mesh stored frames and sparse vertices in two
    // arrays directly on the Mesh. The optional name and bounds live on frames.
    if !matches!(root.get("m_Shapes")?, Some(unityrs_core::Value::Dict(_))) {
        return Ok(BlendShapes {
            vertices: root.collect("m_ShapeVertices", vertex)?,
            frames: root.collect("m_Shapes", frame)?,
            channels: MeteredVec::new(root.meter),
            full_weights: MeteredVec::new(root.meter),
        });
    }
    let shapes = root
        .class("m_Shapes")?
        .ok_or(MeshError::Field("m_Shapes"))?;
    Ok(BlendShapes {
        vertices: shapes.collect("vertices", vertex)?,
        frames: shapes.collect("shapes", frame)?,
        channels: shapes.collect("channels", |v| {
            Ok(BlendShapeChannel {
                name: v.string("name")?,
                name_hash: v.uint("nameHash")?,
                frame_index: v.int("frameIndex")?,
                frame_count: v.int("frameCount")?,
            })
        })?,
        full_weights: shapes.numbers("fullWeights", float)?,
    })
}
fn vertex(v: Fields<'_>) -> Result<BlendShapeVertex, MeshError> {
    Ok(BlendShapeVertex {
        index: v.uint("index")?,
        vertex: vector(v, "vertex")?,
        normal: vector(v, "normal")?,
        tangent: vector(v, "tangent")?,
    })
}
fn frame(v: Fields<'_>) -> Result<BlendShapeFrame, MeshError> {
    Ok(BlendShapeFrame {
        first_vertex: v.uint("firstVertex")?,
        vertex_count: v.uint("vertexCount")?,
        has_normals: v.boolean("hasNormals")?,
        has_tangents: v.boolean("hasTangents")?,
        name: v.get("name")?.map(|_| v.string("name")).transpose()?,
        aabb_min_delta: v.optional_vec3("aabbMinDelta")?,
        aabb_max_delta: v.optional_vec3("aabbMaxDelta")?,
    })
}
fn vector(v: Fields<'_>, name: &'static str) -> Result<[f32; 3], MeshError> {
    v.class(name)?.ok_or(MeshError::Field(name))?.vec3()
}

pub(crate) fn lod(root: Fields<'_>) -> Result<Option<MeshLods>, MeshError> {
    root.class("m_MeshLodInfo")?
        .map(|v| {
            let curve = v
                .class("m_LodSelectionCurve")?
                .ok_or(MeshError::Field("m_LodSelectionCurve"))?;
            Ok(MeshLods {
                num_levels: v.uint("m_NumLevels")?,
                lod_bias: curve.float("m_LodBias")?,
                lod_slope: curve.float("m_LodSlope")?,
                submeshes: v.collect("m_SubMeshes", |s| {
                    Ok(MeshLodSubMesh {
                        levels: s.collect("m_Levels", |r| {
                            Ok(MeshLodRange {
                                index_start: r.uint("m_IndexStart")?,
                                index_count: r.uint("m_IndexCount")?,
                            })
                        })?,
                    })
                })?,
            })
        })
        .transpose()
}
