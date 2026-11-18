//! A Unity 6000.6 Mesh, which no longer has `m_MeshUsageFlags` and adds the
//! two `m_PreBake*` flags (Tier C, `PARITY.md` section 19.1). Every tree and
//! value is built here through the testkit.
use std::sync::Arc;
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_meshdecode::{MeshError, MeshHandler, Source, Version};
use unityrs_testkit::ngui::V;
use unityrs_testkit::scene3d::mesh::{MeshVersion, mesh};
use unityrs_testkit::scene3d::{self, Layout};

const VERTICES: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
const INDICES: [u32; 6] = [0, 1, 2, 2, 1, 3];

/// A Mesh in the 6000.6 field list: without `m_MeshUsageFlags` unless
/// `usage_flags`, with the `m_PreBake*` flags and `m_MeshLodInfo`.
fn unity6_mesh(usage_flags: bool) -> unityrs_testkit::ngui::Object {
    let mut object = mesh(7, b"quad", &VERTICES, &INDICES, MeshVersion::V6000);
    let V::Struct(_, fields) = &mut object.value else {
        unreachable!("a Mesh is a class")
    };
    if !usage_flags {
        fields.retain(|(name, _)| name != "m_MeshUsageFlags");
    }
    let at = fields.iter().position(|(n, _)| n == "m_LocalAABB").unwrap() + 1;
    for name in [
        "m_PreBakeTriangleCollisionMesh",
        "m_PreBakeConvexCollisionMesh",
    ] {
        fields.insert(at, (name.to_owned(), V::Bool(false)));
    }
    object
}

fn process(layout: Layout, usage_flags: bool) -> Result<(u64, usize, usize, usize), MeshError> {
    let data = scene3d::file(layout, &[], &[unity6_mesh(usage_flags)]).unwrap();
    let meter = Meter::unlimited();
    let file = SerializedFile::parse(
        RawStr::from_static(b"CAB-mesh"),
        Bytes::from(data),
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let parsed = file.object(7).unwrap().parse_as_dict(true).unwrap();
    let version = Version::of(&file.version().unwrap().version);
    let processed = MeshHandler::new(Source::mesh(&parsed), version).process(None, &meter)?;
    let triangles = processed.get_triangles(&meter)?;
    Ok((
        processed.vertex_count(),
        processed.index_buffer().map_or(0, |b| b.as_slice().len()),
        triangles.submeshes().len(),
        triangles
            .submeshes()
            .iter()
            .map(|s| s.triangles().len())
            .sum(),
    ))
}

#[test]
fn a_unity_6000_mesh_without_usage_flags_decodes() {
    assert_eq!(process(Layout::Unity6000, false).unwrap(), (4, 6, 1, 2));
    // The flag, where a 6000 file still has it, changes nothing.
    assert_eq!(process(Layout::Unity6000, true).unwrap(), (4, 6, 1, 2));
}

#[test]
fn before_6000_a_mesh_without_usage_flags_is_still_unitypys_unknown_object() {
    // UnityPy 1.25.2's Mesh requires the field: object mode builds an
    // UnknownObject, and process raises (helpers/MeshHelper.py:171-172).
    for layout in [Layout::Unity2019, Layout::Unity2022] {
        assert!(matches!(
            process(layout, false),
            Err(MeshError::UnknownMeshType)
        ));
        assert_eq!(process(layout, true).unwrap(), (4, 6, 1, 2));
    }
}
