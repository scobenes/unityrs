//! The object read and geometry decode, followed by independent metadata owners.
use crate::fields::{Fields, int, uint, work};
use crate::{Mesh, MeshError, SubMesh};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::Environment;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_meshdecode::{FileResources, MeshHandler, Source, Version};

/// Reads a Mesh without choosing export policy or changing its coordinates.
///
/// The core parses on the source file's meter; all result buffers and geometry
/// decoding are charged to `meter`. Geometry errors, including `get_triangles`'
/// refusal of lines/points or an absent index buffer, retain meshdecode's semantics.
/// Bind poses retain partial compressed matrices as trailing scalar values.
/// Every allocation made here is fallible; core's existing CR10 allocation sites
/// remain the core's responsibility (docs/plans/batch.md section 6).
pub fn read_mesh(
    obj: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Mesh, MeshError> {
    if obj.class_id() != 43 {
        return Err(MeshError::NotMesh);
    }
    let parsed = obj.parse_as_dict(true)?;
    let version = Version::of(
        &obj.file()
            .version()
            .ok_or(MeshError::Field("version"))?
            .version,
    );
    let resources = FileResources {
        env,
        file: obj.file(),
    };
    let processed =
        MeshHandler::new(Source::mesh(&parsed), version).process(Some(&resources), meter)?;
    let triangles = processed.get_triangles(meter)?;
    let processed = processed.into_owned();
    let root = Fields::new(&parsed, parsed.value(), meter)?;
    let name = root.string("m_Name")?;
    let submeshes = root.collect("m_SubMeshes", |sub| {
        Ok(SubMesh {
            first_byte: sub.uint("firstByte")?,
            index_count: sub.uint("indexCount")?,
            topology: sub
                .get("topology")?
                .map(|v| int(v, "topology"))
                .transpose()?,
            base_vertex: sub
                .get("baseVertex")?
                .map(|v| uint(v, "baseVertex"))
                .transpose()?,
            first_vertex: sub.uint("firstVertex")?,
            vertex_count: sub.uint("vertexCount")?,
            bounds: sub.bounds("localAABB")?,
        })
    })?;
    let bounds = root.bounds("m_LocalAABB")?;
    let bind_poses = if let Some(array) = processed.bind_pose() {
        let mut out = MeteredVec::with_capacity(meter, array.len().saturating_mul(16))?;
        work(meter, array.len(), 64)?;
        for i in 0..array.len() {
            for j in 0..array.tuple_len(i).unwrap_or(0) {
                let Some(value) = array.component(i, j) else {
                    continue;
                };
                // The Mesh model stores Unity's f32 matrices; the handler keeps
                // its f64 values alongside them for L6 and raw OBJ parity.
                #[allow(clippy::cast_possible_truncation)]
                {
                    out.push(value.as_f64() as f32)?;
                }
            }
        }
        out
    } else {
        let items = root.items("m_BindPose")?;
        let mut out = MeteredVec::with_capacity(meter, items.len().saturating_mul(16))?;
        work(meter, items.len(), 64)?;
        for value in items {
            out.extend_from_slice(&matrix(root.child(value)?)?)?;
        }
        out
    };
    Ok(Mesh {
        name,
        processed,
        submeshes,
        triangles,
        bounds,
        bind_poses,
        bone_name_hashes: root.numbers("m_BoneNameHashes", uint)?,
        root_bone_name_hash: root
            .get("m_RootBoneNameHash")?
            .map(|v| uint(v, "m_RootBoneNameHash"))
            .transpose()?,
        blend_shapes: crate::shapes::read(root)?,
        lod: crate::shapes::lod(root)?,
    })
}

fn matrix(fields: Fields<'_>) -> Result<[f32; 16], MeshError> {
    const NAMES: [&str; 16] = [
        "e00", "e01", "e02", "e03", "e10", "e11", "e12", "e13", "e20", "e21", "e22", "e23", "e30",
        "e31", "e32", "e33",
    ];
    let mut matrix = [0.0; 16];
    for (value, name) in matrix.iter_mut().zip(NAMES) {
        *value = fields.float(name)?;
    }
    Ok(matrix)
}
