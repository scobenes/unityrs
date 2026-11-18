//! A Unity 6000.6 Mesh, which no longer has `m_MeshUsageFlags` and adds the
//! two `m_PreBake*` flags (Tier C, `PARITY.md` section 19.1). Every tree and
//! value is built here through the testkit.
use std::sync::Arc;
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_meshdecode::{MeshError, MeshHandler, Source, Version};
use unityrs_testkit::ngui::{Object, V};
use unityrs_testkit::scene3d::mesh::{MeshVersion, mesh};
use unityrs_testkit::scene3d::{self, Layout};

const VERTICES: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
const INDICES: [u32; 6] = [0, 1, 2, 2, 1, 3];

/// A Mesh in the 6000.6 field list: without `m_MeshUsageFlags` unless
/// `usage_flags`, with the `m_PreBake*` flags and `m_MeshLodInfo`.
fn unity6_mesh(usage_flags: bool) -> Object {
    let mut object = mesh(7, b"quad", &VERTICES, &INDICES, MeshVersion::V6000);
    let fields = fields(&mut object);
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

fn fields(object: &mut Object) -> &mut Vec<(String, V)> {
    let V::Struct(_, fields) = &mut object.value else {
        unreachable!("a Mesh is a class")
    };
    fields
}

/// The file of `layout`, its engine version spelled `engine` (as long as
/// the layout's own).
fn input(layout: Layout, engine: &[u8], object: &Object) -> Bytes {
    let mut data = scene3d::file(layout, &[], std::slice::from_ref(object)).unwrap();
    let own = layout.version();
    assert_eq!(own.len(), engine.len());
    let at = data.windows(own.len()).position(|w| w == own).unwrap();
    data[at..at + own.len()].copy_from_slice(engine);
    Bytes::from(data)
}

/// Positions, indices and triangles per submesh, or the first error.
type Decoded = (Vec<[f64; 3]>, Vec<u32>, Vec<Vec<[u32; 3]>>);

fn decode(data: Bytes, version: Option<Version>) -> Result<Decoded, MeshError> {
    let meter = Meter::unlimited();
    let file = SerializedFile::parse(
        RawStr::from_static(b"CAB-mesh"),
        data,
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let parsed = file.object(7).unwrap().parse_as_dict(true).unwrap();
    let version = version.unwrap_or_else(|| Version::of(&file.version().unwrap().version));
    let processed = MeshHandler::new(Source::mesh(&parsed), version).process(None, &meter)?;
    let vertices = processed.vertices().unwrap();
    let positions = (0..vertices.len())
        .map(|v| [0, 1, 2].map(|c| vertices.component(v, c).unwrap().as_f64()))
        .collect();
    let indices = processed.index_buffer().unwrap().as_slice().to_vec();
    let triangles = processed.get_triangles(&meter)?;
    let triangles = triangles
        .submeshes()
        .iter()
        .map(|s| s.triangles().to_vec())
        .collect();
    Ok((positions, indices, triangles))
}

fn expected() -> Decoded {
    (
        VERTICES.iter().map(|v| v.map(f64::from)).collect(),
        INDICES.to_vec(),
        vec![vec![[0, 1, 2], [2, 1, 3]]],
    )
}

#[test]
fn a_unity_6000_mesh_without_usage_flags_decodes() {
    for usage_flags in [false, true] {
        // The flag, where a 6000 file still has it, changes nothing.
        let data = input(Layout::Unity6000, b"6000.6.9f1", &unity6_mesh(usage_flags));
        assert_eq!(decode(data, None).unwrap(), expected());
    }
}

#[test]
fn before_6000_a_mesh_without_usage_flags_is_still_unitypys_unknown_object() {
    // UnityPy 1.25.2's Mesh requires the field: object mode builds an
    // UnknownObject, and process raises (helpers/MeshHelper.py:171-172).
    for layout in [Layout::Unity2019, Layout::Unity2022] {
        let data = |flags| input(layout, layout.version(), &unity6_mesh(flags));
        assert!(matches!(
            decode(data(false), None),
            Err(MeshError::UnknownMeshType)
        ));
        assert_eq!(decode(data(true), None).unwrap(), expected());
    }
}

#[test]
fn the_vertex_data_assertion_precedes_the_version_comparison() {
    // A type byte that is no UnityVersionType raises at the first version
    // comparison; m_VertexData is asserted before it, flagged or not.
    let bad = Version::new(6000, 6, 9, 7, 1);
    for usage_flags in [false, true] {
        let mut object = unity6_mesh(usage_flags);
        let data = input(Layout::Unity6000, b"6000.6.9f1", &object);
        assert!(matches!(
            decode(data, Some(bad)),
            Err(MeshError::VersionType(7))
        ));
        fields(&mut object).retain(|(name, _)| name != "m_VertexData");
        let data = input(Layout::Unity6000, b"6000.6.9f1", &object);
        let got = decode(data, Some(bad));
        if usage_flags {
            // A built Mesh: the field's default, None, fails the assert.
            assert!(matches!(got, Err(MeshError::Assertion(_))), "{got:?}");
        } else {
            // 1.25.2's UnknownObject has no such attribute.
            assert!(matches!(got, Err(MeshError::NoAttribute(_))), "{got:?}");
        }
    }
}
