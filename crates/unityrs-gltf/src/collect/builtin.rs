//! Opt-in approximate primitives; no Editor resource data is embedded.
use super::{Arc, Meter, MeteredVec, Result, Scene, extra, invalid, number};
use crate::{Accessor, Attribute, Data, Kind, Mesh, Primitive};
use unityrs_scene::builtin::BuiltinKind;
pub(super) fn procedural(
    scene: &mut Scene,
    kind: Option<BuiltinKind>,
    meter: &Arc<Meter>,
) -> Result<usize> {
    let (positions, indices): (&[[f64; 3]], &[u16]) = match kind {
        Some(BuiltinKind::Quad) => (
            &[
                [-0.5, -0.5, 0.],
                [0.5, -0.5, 0.],
                [0.5, 0.5, 0.],
                [-0.5, 0.5, 0.],
            ],
            &[0, 1, 2, 0, 2, 3],
        ),
        Some(BuiltinKind::Plane) => (
            &[[-5., 0., -5.], [-5., 0., 5.], [5., 0., 5.], [5., 0., -5.]],
            &[0, 1, 2, 0, 2, 3],
        ),
        Some(BuiltinKind::Cube) => (
            &[
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
            &[
                0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0,
                7, 3, 1, 2, 6, 1, 6, 5,
            ],
        ),
        _ => return Err(invalid("procedural builtin supports Cube, Quad and Plane")),
    };
    meter.count_decode((positions.len() * 24 + indices.len() * 2) as u64)?;
    let mut values = MeteredVec::with_capacity(meter, positions.len() * 3)?;
    for &[x, y, z] in positions {
        values.extend_from_slice(&[-x, y, z])?;
    }
    let pos = scene.accessors.len();
    scene.accessors.push(Accessor {
        kind: Kind::Position,
        count: positions.len(),
        data: Data::Float(values),
        sparse_indices: None,
    })?;
    let mut values = MeteredVec::with_capacity(meter, indices.len())?;
    for &[a, b, c] in indices.as_chunks::<3>().0 {
        values.extend_from_slice(&[c, b, a])?;
    }
    let ind = scene.accessors.len();
    scene.accessors.push(Accessor {
        kind: Kind::Indices,
        count: indices.len(),
        data: Data::U16(values),
        sparse_indices: None,
    })?;
    let mut primitives = MeteredVec::new(meter);
    primitives.push(Primitive {
        attributes: crate::array(meter, &[(Attribute::Position, pos)])?,
        indices: Some(ind),
        material: None,
        targets: MeteredVec::new(meter),
        extras: MeteredVec::new(meter),
    })?;
    let mut extras = MeteredVec::new(meter);
    extra(
        &mut extras,
        meter,
        "approximate",
        crate::text(meter, "true")?,
    )?;
    let id = scene.meshes.len();
    scene.meshes.push(Mesh {
        name: None,
        primitives,
        target_names: MeteredVec::new(meter),
        extras,
    })?;
    Ok(id)
}
pub(super) fn mark(scene: &mut Scene, id: usize, path_id: i64, meter: &Arc<Meter>) -> Result<()> {
    super::diagnostics::merge(
        &mut scene
            .nodes
            .as_mut_slice()
            .get_mut(id)
            .ok_or_else(|| invalid("builtin node"))?
            .extras,
        meter,
        "unity_builtin",
        number(meter, path_id)?.as_bytes(),
    )
}

pub(super) fn resolve(
    env: &unityrs_core::env::Environment,
    builtin: &unityrs_scene::builtin::BuiltinRef,
    meter: &Arc<Meter>,
) -> Result<unityrs_core::ObjectRef> {
    let file = env
        .find_file(&builtin.path, true, meter)
        .map_err(|e| super::GltfError::Mesh(unityrs_mesh::MeshError::Core(e)))?;
    let Some(unityrs_core::files::File::Serialized(file)) = file else {
        return Err(invalid("builtin resource unresolved"));
    };
    file.object(builtin.path_id)
        .ok_or_else(|| invalid("builtin object unresolved"))
}
