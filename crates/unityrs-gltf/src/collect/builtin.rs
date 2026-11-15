//! Opt-in approximate primitives; no Editor resource data is embedded.
use super::{Arc, Meter, MeteredVec, Result, Scene, extra, invalid};
use crate::{Accessor, Attribute, BuiltinIdentity, Data, Kind, Mesh, Primitive};
use unityrs_scene::builtin::{BuiltinFile, BuiltinKind, BuiltinRef};
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
pub(super) fn mark(
    scene: &mut Scene,
    id: usize,
    builtin: &BuiltinRef,
    meter: &Arc<Meter>,
) -> Result<()> {
    let identities = &mut scene
        .nodes
        .as_mut_slice()
        .get_mut(id)
        .ok_or_else(|| invalid("builtin node"))?
        .builtins;
    let identity = BuiltinIdentity {
        file: builtin.file.name(),
        id: builtin.path_id,
        name: builtin.kind.map(BuiltinKind::name),
    };
    meter.count_decode(identities.len() as u64 * 64 + 1)?;
    let Err(at) = identities.as_slice().binary_search(&identity) else {
        return Ok(());
    };
    identities.push(identity)?;
    identities
        .as_mut_slice()
        .get_mut(at..)
        .ok_or_else(|| invalid("builtin identity insertion"))?
        .rotate_right(1);
    Ok(())
}

/// Resolved resource files are retained only for this collection. In
/// particular, arbitrary resolver source names must not reload the same file
/// per renderer or be promoted to permanent registry aliases.
pub(crate) struct Cache {
    files: MeteredVec<(BuiltinFile, Arc<unityrs_core::files::SerializedFile>)>,
}
impl Cache {
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            files: MeteredVec::new(meter),
        }
    }
    pub(crate) fn resolve(
        &mut self,
        env: &unityrs_core::env::Environment,
        builtin: &BuiltinRef,
        meter: &Arc<Meter>,
    ) -> Result<unityrs_core::ObjectRef> {
        meter.count_decode(builtin.path.len() as u64 + self.files.len() as u64 + 1)?;
        let key = BuiltinFile::from_path(builtin.path.as_bytes())
            .ok_or_else(|| invalid("builtin resource name"))?;
        if let Some((_, file)) = self.files.as_slice().iter().find(|(old, _)| *old == key) {
            return file
                .object(builtin.path_id)
                .ok_or_else(|| invalid("builtin object unresolved"));
        }
        let file = env
            .find_file(
                &unityrs_core::RawStr::from_static(key.name().as_bytes()),
                true,
                meter,
            )
            .map_err(|e| super::GltfError::Mesh(unityrs_mesh::MeshError::Core(e)))?;
        let Some(unityrs_core::files::File::Serialized(file)) = file else {
            return Err(invalid("builtin resource unresolved"));
        };
        let object = file.object(builtin.path_id);
        self.files.push((key, file))?;
        object.ok_or_else(|| invalid("builtin object unresolved"))
    }
}
const _: () = crate::send_sync::<Cache>();
