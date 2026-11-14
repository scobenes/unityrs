use unityrs_core::limits::Meter;
use unityrs_mesh::read_mesh;
use unityrs_testkit::ngui::{V, set_field, st, vector};
use unityrs_testkit::scene3d::mesh::{self, MeshVersion};

mod support;

#[test]
#[allow(clippy::too_many_lines)]
fn every_mesh_field_survives_the_owned_model() {
    for version in [MeshVersion::V2019, MeshVersion::V2022, MeshVersion::V6000] {
        let mut obj = mesh::mesh(
            1,
            b"m\xffesh",
            &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            &[0, 1, 2],
            version,
        );
        obj = set_field(
            obj,
            "m_LocalAABB",
            mesh::aabb([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]),
            None,
        );
        let mut matrix = [0.0; 16];
        for (i, x) in matrix.iter_mut().enumerate() {
            *x = f32::from(u16::try_from(i).unwrap()) + 0.5;
        }
        obj = set_field(
            obj,
            "m_BindPose",
            vector(mesh::matrix([0.0; 16]), vec![mesh::matrix(matrix)]),
            None,
        );
        obj = set_field(
            obj,
            "m_BoneNameHashes",
            vector(V::UInt(0), vec![V::UInt(17), V::UInt(u32::MAX)]),
            None,
        );
        obj = set_field(obj, "m_RootBoneNameHash", V::UInt(42), None);
        let vertices = vec![
            mesh::blend_vertex(2, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]),
            mesh::blend_vertex(0, [-1.0, -2.0, -3.0], [0.0; 3], [0.0; 3]),
        ];
        let frames = vec![
            mesh::blend_frame(0, 1, true, false),
            mesh::blend_frame(1, 1, false, true),
        ];
        let channels = vec![mesh::blend_channel(b"b\xffend", 123, 0, 2)];
        obj = set_field(
            obj,
            "m_Shapes",
            mesh::blend_shapes(vertices, frames, channels, vec![25.0, 100.0]),
            None,
        );
        if version == MeshVersion::V6000 {
            obj = set_field(
                obj,
                "m_MeshLodInfo",
                mesh::lod_info(2, 1.5, -2.5, &[vec![(0, 3), (3, 6)], vec![(9, 12)]]),
                None,
            );
        }
        let (env, file) = support::load(version, &[obj]);
        let meter = Meter::unlimited();
        let model = read_mesh(&file.object(1).unwrap(), &env, &meter).unwrap();
        drop(file);
        drop(env);
        assert_eq!(model.name.as_bytes(), b"m\xffesh");
        assert_eq!(model.processed.vertex_count(), 3);
        assert_eq!(
            model.processed.vertices().unwrap().component(2, 1),
            Some(unityrs_meshdecode::Component::Float(8.0))
        );
        assert_eq!(
            model.processed.index_buffer().unwrap().as_slice(),
            &[0, 1, 2]
        );
        assert_eq!(model.triangles.submeshes()[0].triangles(), &[[0, 1, 2]]);
        let sub = &model.submeshes.as_slice()[0];
        assert_eq!(
            (
                sub.first_byte,
                sub.index_count,
                sub.topology,
                sub.base_vertex,
                sub.first_vertex,
                sub.vertex_count
            ),
            (0, 3, Some(0), Some(0), 0, 3)
        );
        assert_eq!(sub.bounds.unwrap().center, [0.0; 3]);
        assert_eq!(sub.bounds.unwrap().extent, [1.0; 3]);
        let bounds = model.bounds.unwrap();
        assert_eq!(
            (bounds.center, bounds.extent),
            ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0])
        );
        assert_eq!(model.bind_poses.as_slice(), &matrix);
        assert_eq!(model.bone_name_hashes.as_slice(), &[17, u32::MAX]);
        assert_eq!(model.root_bone_name_hash, Some(42));
        let shapes = &model.blend_shapes;
        assert_eq!(shapes.vertices.len(), 2);
        assert_eq!(shapes.vertices.as_slice()[0].index, 2);
        assert_eq!(shapes.vertices.as_slice()[0].vertex, [1.0, 2.0, 3.0]);
        assert_eq!(shapes.vertices.as_slice()[0].normal, [4.0, 5.0, 6.0]);
        assert_eq!(shapes.vertices.as_slice()[0].tangent, [7.0, 8.0, 9.0]);
        let frame = &shapes.frames.as_slice()[0];
        assert_eq!(
            (
                frame.first_vertex,
                frame.vertex_count,
                frame.has_normals,
                frame.has_tangents
            ),
            (0, 1, true, false)
        );
        assert_eq!(
            (
                frame.name.as_ref().map(unityrs_mesh::RawString::as_bytes),
                frame.aabb_min_delta,
                frame.aabb_max_delta
            ),
            (None, None, None)
        );
        let second = &shapes.frames.as_slice()[1];
        assert_eq!(
            (
                second.first_vertex,
                second.vertex_count,
                second.has_normals,
                second.has_tangents
            ),
            (1, 1, false, true)
        );
        let channel = &shapes.channels.as_slice()[0];
        assert_eq!(
            (
                channel.name.as_bytes(),
                channel.name_hash,
                channel.frame_index,
                channel.frame_count
            ),
            (b"b\xffend".as_slice(), 123, 0, 2)
        );
        assert_eq!(shapes.full_weights.as_slice(), &[25.0, 100.0]);
        if version == MeshVersion::V6000 {
            let lod = model.lod.as_ref().unwrap();
            assert_eq!(
                (lod.num_levels, lod.lod_bias, lod.lod_slope),
                (2, 1.5, -2.5)
            );
            assert_eq!(lod.submeshes.len(), 2);
            assert_eq!(
                lod.submeshes.as_slice()[0]
                    .levels
                    .as_slice()
                    .iter()
                    .map(|r| (r.index_start, r.index_count))
                    .collect::<Vec<_>>(),
                vec![(0, 3), (3, 6)]
            );
            let range = lod.submeshes.as_slice()[1].levels.as_slice()[0];
            assert_eq!((range.index_start, range.index_count), (9, 12));
        } else {
            assert!(model.lod.is_none());
        }
        assert!(meter.used() > 0);
        drop(model);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn optional_and_legacy_frame_fields_are_preserved() {
    let mut obj = mesh::mesh(1, b"empty", &[], &[], MeshVersion::V2022);
    // An empty index buffer gives get_triangles its ordinary assertion refusal.
    let (env, file) = support::load(MeshVersion::V2022, &[obj.clone()]);
    assert!(read_mesh(&file.object(1).unwrap(), &env, &Meter::unlimited()).is_err());
    obj = mesh::mesh(
        1,
        b"legacy-frame",
        &[[0.0; 3]; 3],
        &[0, 1, 2],
        MeshVersion::V2019,
    );
    let frame = st(
        "MeshBlendShape",
        vec![
            ("firstVertex", V::UInt(0)),
            ("vertexCount", V::UInt(0)),
            ("hasNormals", V::Bool(false)),
            ("hasTangents", V::Bool(false)),
            ("name", V::Str(b"old".to_vec())),
            ("aabbMinDelta", mesh::vec3([-3.0; 3])),
            ("aabbMaxDelta", mesh::vec3([4.0; 3])),
        ],
    );
    obj = set_field(
        obj,
        "m_Shapes",
        mesh::blend_shapes(vec![], vec![frame], vec![], vec![50.0]),
        None,
    );
    let (env, file) = support::load(MeshVersion::V2019, &[obj]);
    let model = read_mesh(&file.object(1).unwrap(), &env, &Meter::unlimited()).unwrap();
    let frame = &model.blend_shapes.frames.as_slice()[0];
    assert_eq!(frame.name.as_ref().unwrap().as_bytes(), b"old");
    assert_eq!(
        (frame.aabb_min_delta, frame.aabb_max_delta),
        (Some([-3.0; 3]), Some([4.0; 3]))
    );
    assert!(model.bind_poses.is_empty());
    assert!(model.bone_name_hashes.is_empty());
    assert_eq!(model.root_bone_name_hash, Some(0));
    assert!(model.blend_shapes.channels.is_empty());
}

#[test]
fn legacy_shape_arrays_keep_their_named_frames_and_sparse_vertices() {
    let frame = st(
        "MeshBlendShape",
        vec![
            ("firstVertex", V::UInt(0)),
            ("vertexCount", V::UInt(1)),
            ("hasNormals", V::Bool(true)),
            ("hasTangents", V::Bool(false)),
            ("name", V::Str(b"legacy".to_vec())),
            ("aabbMinDelta", mesh::vec3([-1.0; 3])),
            ("aabbMaxDelta", mesh::vec3([2.0; 3])),
        ],
    );
    let mut obj = mesh::mesh(
        1,
        b"old-shapes",
        &[[0.0; 3]; 3],
        &[0, 1, 2],
        MeshVersion::V2019,
    );
    obj = set_field(obj, "m_Shapes", vector(frame.clone(), vec![frame]), None);
    obj = set_field(
        obj,
        "m_ShapeVertices",
        vector(
            mesh::blend_vertex(0, [0.0; 3], [0.0; 3], [0.0; 3]),
            vec![mesh::blend_vertex(2, [1.0; 3], [2.0; 3], [3.0; 3])],
        ),
        None,
    );
    let (env, file) = support::load(MeshVersion::V2019, &[obj]);
    let meter = Meter::unlimited();
    let model = read_mesh(&file.object(1).unwrap(), &env, &meter).unwrap();
    assert_eq!(model.blend_shapes.vertices.as_slice()[0].index, 2);
    assert_eq!(
        model.blend_shapes.frames.as_slice()[0]
            .name
            .as_ref()
            .unwrap()
            .as_bytes(),
        b"legacy"
    );
    assert!(model.blend_shapes.channels.is_empty());
    assert!(model.blend_shapes.full_weights.is_empty());
    drop(model);
    assert_eq!(meter.used(), 0);
}

#[test]
fn absent_optional_metadata_stays_absent_and_ranges_are_not_rewritten() {
    let mut obj = mesh::mesh(
        1,
        b"optional",
        &[[0.0; 3]; 3],
        &[0, 1, 2],
        MeshVersion::V2019,
    );
    if let V::Struct(_, fields) = &mut obj.value {
        fields.retain(|(n, _)| {
            !["m_Shapes", "m_BoneNameHashes", "m_RootBoneNameHash"].contains(&n.as_str())
        });
    }
    obj = set_field(
        obj,
        "m_SubMeshes",
        vector(
            mesh::submesh(0, 3, 0, 7, 8, 9),
            vec![mesh::submesh(0, 3, 0, 7, 8, 9)],
        ),
        None,
    );
    let (env, file) = support::load(MeshVersion::V2019, &[obj]);
    let m = read_mesh(&file.object(1).unwrap(), &env, &Meter::unlimited()).unwrap();
    assert!(
        m.blend_shapes.vertices.is_empty()
            && m.blend_shapes.frames.is_empty()
            && m.blend_shapes.channels.is_empty()
            && m.blend_shapes.full_weights.is_empty()
    );
    assert!(m.bone_name_hashes.is_empty());
    assert!(m.root_bone_name_hash.is_none());
    assert!(m.lod.is_none());
    let s = m.submeshes.as_slice()[0];
    assert_eq!(
        (s.base_vertex, s.first_vertex, s.vertex_count),
        (Some(7), 8, 9)
    );
    assert_eq!(m.triangles.submeshes()[0].triangles(), &[[0, 1, 2]]);
}
