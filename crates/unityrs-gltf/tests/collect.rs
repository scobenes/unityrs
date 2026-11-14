use unityrs_gltf::*;
use unityrs_testkit::{
    ngui::{Object, V},
    scene3d as s,
};
mod collect_support;
use collect_support as h;
#[test]
fn coordinates_submeshes_and_sparse_last_frame() {
    let scene = h::collect(false, false);
    assert_eq!(scene.meshes.len(), 1);
    let mesh = &scene.meshes.as_slice()[0];
    assert_eq!(mesh.primitives.len(), 2);
    let p = &mesh.primitives.as_slice()[0];
    for (attr, expected) in [
        (
            Attribute::Position,
            vec![-1., 0., 0., 0., 1., 0., 0., 0., 0.],
        ),
        (Attribute::Normal, vec![0., 0., 1., 0., 0., 1., 0., 0., 1.]),
        (
            Attribute::Tangent,
            vec![-1., 0., 0., -1., -1., 0., 0., -1., -1., 0., 0., -1.],
        ),
    ] {
        let id = p
            .attributes
            .as_slice()
            .iter()
            .find(|(k, _)| *k == attr)
            .unwrap()
            .1;
        assert_eq!(h::floats(&scene, id), expected);
    }
    let uv = p
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == Attribute::TexCoord(0))
        .unwrap()
        .1;
    assert_eq!(h::floats(&scene, uv)[1], 0.75);
    let Data::U16(indices) = &scene.accessors.as_slice()[p.indices.unwrap()].data else {
        panic!("indices")
    };
    assert_eq!(indices.as_slice(), [2, 1, 0]);
    let morph = &scene.accessors.as_slice()[p.targets.as_slice()[0][0].unwrap()];
    assert_eq!(morph.sparse_indices.as_ref().unwrap().as_slice(), [2]);
    assert_eq!(
        h::floats(&scene, p.targets.as_slice()[0][0].unwrap()),
        [-2., 0., 0.]
    );
    assert_ne!(h::write(&scene), [] as [u8; 0]);
}
#[test]
fn skin_has_identity_root_and_joints_keep_world_transforms() {
    let scene = h::collect(true, false);
    let (id, node) = scene
        .nodes
        .as_slice()
        .iter()
        .enumerate()
        .find(|(_, n)| n.skin.is_some())
        .unwrap();
    assert!(scene.roots.as_slice().contains(&id));
    assert_eq!(node.translation, [0.; 3]);
    assert_eq!(node.rotation, [0., 0., 0., 1.]);
    assert_eq!(node.scale, [1.; 3]);
    assert!(
        scene
            .nodes
            .as_slice()
            .iter()
            .all(|n| !n.children.as_slice().contains(&id))
    );
    let joint = scene.skins.as_slice()[0].joints.as_slice()[0];
    assert_eq!(scene.nodes.as_slice()[joint].translation, [-5., 2., 0.]);
    // Identity inverse bind, a unit joint weight: mirrored vertex + joint world translation.
    assert_eq!(
        h::floats(&scene, 0)[0] + scene.nodes.as_slice()[joint].translation[0],
        -6.
    );
    assert_eq!(node.weights.as_slice(), [0.25]);
    h::write(&scene);
}
#[test]
fn batching_uses_world_space_root_and_range() {
    let scene = h::collect(false, true);
    let (id, node) = scene
        .nodes
        .as_slice()
        .iter()
        .enumerate()
        .find(|(_, n)| n.mesh.is_some())
        .unwrap();
    assert!(scene.roots.as_slice().contains(&id));
    assert_eq!(node.translation, [0.; 3]);
    assert_eq!(
        scene.meshes.as_slice()[node.mesh.unwrap()].primitives.len(),
        1
    );
    assert_eq!(scene.nodes.as_slice()[1].translation, [-2., 0., 0.]);
    h::write(&scene);
}
#[test]
fn inactive_layers_and_lod_only_suppress_selected_content() {
    let mut objects = h::objects(false, false);
    s::set(&mut objects[2].value, "m_IsActive", V::Bool(false));
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = Profile::new(&m);
    assert_eq!(collect(&env, &g, &p, &m).unwrap().nodes.len(), 1);
    p.inactive = Inactive::IncludeHidden;
    let scene = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(scene.nodes.len(), 2);
    h::write(&scene);
    p.layer_skips.push(0).unwrap();
    assert!(collect(&env, &g, &p, &m).unwrap().nodes.is_empty());
    let mut objects = h::objects(false, false);
    s::set(
        &mut objects[0].value,
        "m_Component",
        s::pointers("Component", &[(0, 2)]),
    );
    // Restore ComponentPair shape and add LODGroup in the parent's components.
    objects[0] = Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 8)]));
    objects.push(Object::new(
        8,
        205,
        s::lod_group((0, 1), &[(0.8, vec![]), (0.2, vec![(0, 6)])]),
    ));
    let (env, g) = h::graph(&objects);
    let mut p = Profile::new(&m);
    assert!(collect(&env, &g, &p, &m).unwrap().meshes.is_empty());
    p.lod = Lod::All;
    assert_eq!(collect(&env, &g, &p, &m).unwrap().meshes.len(), 1);
}

#[test]
fn builtins_placeholder_procedural_and_resolve_are_explicit() {
    use unityrs_core::{
        Bytes, RawStr,
        env::{EnvOptions, Environment, Source},
        files::File,
        limits::Meter,
    };
    use unityrs_scene::{GraphOptions, graph_from_scene_file};
    let mut objects = h::objects(false, false);
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"builtin-scene"),
            data: Bytes::from(
                s::file(
                    s::Layout::Unity2022,
                    &["Library/unity default resources"],
                    &objects,
                )
                .unwrap(),
            ),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let m = Meter::unlimited();
    let g = graph_from_scene_file(&env, &file, &GraphOptions::default(), &m).unwrap();
    let mut p = Profile::new(&m);
    let placeholder = collect(&env, &g, &p, &m).unwrap();
    assert!(placeholder.meshes.is_empty());
    assert!(
        placeholder.nodes.as_slice()[1]
            .extras
            .as_slice()
            .iter()
            .any(|(k, _)| k.as_bytes() == b"unity_builtin")
    );
    p.builtin = Builtin::Procedural;
    let scene = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    h::write(&scene);
    p.builtin = Builtin::Resolve;
    let missing = collect(&env, &g, &p, &m).unwrap();
    assert!(missing.meshes.is_empty());
    assert_eq!(missing.collection_issues.len(), 1);
    assert_eq!(
        missing.collection_issues.as_slice()[0].reason,
        "builtin resource unresolved"
    );
    h::write(&missing);
    let mut mesh = s::mesh::mesh(
        10210,
        b"resolved",
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    );
    mesh.path_id = 10210;
    env.load(Source::Bytes {
        name: RawStr::from_static(b"unity default resources"),
        data: Bytes::from(s::file(s::Layout::Unity2022, &[], &[mesh]).unwrap()),
    })
    .unwrap();
    let resolved = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(resolved.meshes.len(), 1);
    h::write(&resolved);
}

#[test]
fn combined_instances_share_accessors_and_keep_combined_draws_once() {
    let mut objects = h::objects(false, true);
    objects[1] = Object::new(
        2,
        4,
        s::transform((0, 1), (0, 0), &[(0, 4), (0, 10)], false),
    );
    objects.extend([
        Object::new(9, 1, s::game_object(b"other", &[(0, 10), (0, 11), (0, 12)])),
        Object::new(10, 4, s::transform((0, 9), (0, 2), &[], false)),
        Object::new(11, 33, s::mesh_filter((0, 9), (0, 7))),
        Object::new(12, 23, s::mesh_renderer((0, 9), &[], (0, 1))),
    ]);
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = Profile::new(&m);
    let unbatched = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(unbatched.meshes.len(), 2);
    assert_eq!(
        unbatched.meshes.as_slice()[0].primitives.as_slice()[0]
            .attributes
            .as_slice(),
        unbatched.meshes.as_slice()[1].primitives.as_slice()[0]
            .attributes
            .as_slice()
    );
    p.static_batch = StaticBatch::KeepCombined;
    let combined = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(
        combined
            .nodes
            .as_slice()
            .iter()
            .filter(|n| n.mesh.is_some())
            .count(),
        1
    );
    assert_eq!(combined.meshes.as_slice()[0].primitives.len(), 2);
    h::write(&combined);
}

#[test]
fn morph_only_renderer_keeps_transform_without_inventing_a_skin() {
    let mut objects = h::objects(true, false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[], &[50.]),
    );
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert!(scene.skins.is_empty());
    let node = scene
        .nodes
        .as_slice()
        .iter()
        .find(|n| n.mesh.is_some())
        .unwrap();
    assert_eq!(node.translation, [-2., 0., 0.]);
    assert_eq!(node.weights.as_slice(), [0.5]);
    h::write(&scene);
}

#[test]
fn skin_instances_share_geometry_and_binds_but_keep_their_weights() {
    let mut objects = h::objects(true, false);
    objects[1] = Object::new(
        2,
        4,
        s::transform((0, 1), (0, 0), &[(0, 4), (0, 10)], false),
    );
    objects.extend([
        Object::new(9, 1, s::game_object(b"other", &[(0, 10), (0, 12)])),
        Object::new(10, 4, s::transform((0, 9), (0, 2), &[], false)),
        Object::new(
            12,
            137,
            s::skinned_mesh_renderer((0, 9), (0, 7), &[(0, 2)], &[75.]),
        ),
    ]);
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.skins.len(), 2);
    assert_eq!(
        scene.skins.as_slice()[0].inverse_bind_matrices,
        scene.skins.as_slice()[1].inverse_bind_matrices
    );
    let weights: Vec<_> = scene
        .nodes
        .as_slice()
        .iter()
        .filter(|n| n.skin.is_some())
        .map(|n| n.weights.as_slice())
        .collect();
    assert_eq!(weights, [&[0.25], &[0.75]]);
    h::write(&scene);
}

#[test]
fn inverse_bind_is_conjugated_and_transposed_for_world_space_skinning() {
    let mut objects = h::objects(true, false);
    let q = std::f32::consts::FRAC_1_SQRT_2;
    s::set(
        &mut objects[1].value,
        "m_LocalRotation",
        s::xyz("Quaternionf", &[0., 0., q, q]),
    );
    s::set(
        &mut objects[1].value,
        "m_LocalScale",
        s::xyz("Vector3f", &[2., 3., 4.]),
    );
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        unityrs_testkit::ngui::vector(
            s::mesh::matrix([0.; 16]),
            vec![s::mesh::matrix([
                1., 0., 0., 3., 0., 1., 0., 4., 0., 0., 1., 5., 0., 0., 0., 1.,
            ])],
        ),
    );
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    let skin = &scene.skins.as_slice()[0];
    let matrix = h::floats(&scene, skin.inverse_bind_matrices.unwrap());
    assert_eq!(
        matrix,
        [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., -3., 4., 5., 1.
        ]
    );
    let joint = &scene.nodes.as_slice()[skin.joints.as_slice()[0]];
    let point = [-1. + matrix[12], matrix[13], matrix[14]];
    let scaled = [
        point[0] * joint.scale[0],
        point[1] * joint.scale[1],
        point[2] * joint.scale[2],
    ];
    // The mirrored quaternion is a -90 degree Z rotation. Independent Unity
    // calculation: (1,0,0)+(3,4,5), scale (2,3,4), rotate +90, add (5,2,0), mirror X.
    let world = [
        scaled[1] + joint.translation[0],
        -scaled[0] + joint.translation[1],
        scaled[2] + joint.translation[2],
    ];
    assert_eq!(world, [7., 10., 20.]);
    assert_eq!(joint.rotation, [0., 0., -f64::from(q), f64::from(q)]);
    h::write(&scene);
}

#[test]
fn normaliser_counts_reach_report_and_asset_extras() {
    let mut objects = h::objects(true, false);
    let V::Struct(_, fields) = &mut objects[6].value else {
        panic!("mesh")
    };
    let (_, V::Struct(_, vertex)) = fields
        .iter_mut()
        .find(|(k, _)| k == "m_VertexData")
        .unwrap()
    else {
        panic!("vertex")
    };
    let (_, V::Bytes(bytes)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap() else {
        panic!("data")
    };
    for row in 0..3 {
        bytes[row * 80 + 20..row * 80 + 24].copy_from_slice(&2f32.to_le_bytes());
        bytes[row * 80 + 36..row * 80 + 40].copy_from_slice(&2f32.to_le_bytes());
        bytes[row * 80 + 48..row * 80 + 52].copy_from_slice(&(-1f32).to_le_bytes());
    }
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let p = Profile::new(&m);
    let scene = collect(&env, &g, &p, &m).unwrap();
    let mut bytes = Vec::new();
    let report = write_glb(&scene, &p.options(), &m, &mut bytes).unwrap();
    assert_eq!(report.normal_components, 3);
    assert_eq!(report.tangent_components, 3);
    assert_eq!(report.zero_weight_rows, 3);
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let doc: serde_json::Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
    assert_eq!(doc["asset"]["extras"]["normal_components"], 3);
    assert_eq!(doc["asset"]["extras"]["tangent_components"], 3);
    assert_eq!(doc["asset"]["extras"]["zero_weight_rows"], 3);
}

#[test]
fn root_table_order_filters_and_required_hidden_joints_are_preserved() {
    let mut objects = h::objects(true, false);
    s::set(&mut objects[0].value, "m_IsActive", V::Bool(false));
    objects[1] = Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false));
    objects[3] = Object::new(4, 4, s::transform((0, 3), (0, 0), &[], false));
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = Profile::new(&m);
    let scene = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(scene.roots.as_slice(), [0, 1, 2]);
    assert!(scene.nodes.as_slice()[0].mesh.is_none());
    h::write(&scene);
    p.scene_roots.push(text(&m, "child").unwrap()).unwrap();
    let scene = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(scene.skins.len(), 1);
    h::write(&scene);
    p.exclusions.push(text(&m, "child").unwrap()).unwrap();
    assert!(collect(&env, &g, &p, &m).unwrap().nodes.is_empty());
    let objects = [
        Object::new(1, 1, s::game_object(b"first", &[(0, 200)])),
        Object::new(200, 4, s::transform((0, 1), (0, 0), &[], false)),
        Object::new(2, 1, s::game_object(b"second", &[(0, -100)])),
        Object::new(-100, 4, s::transform((0, 2), (0, 0), &[], false)),
    ];
    let (env, g) = h::graph(&objects);
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert_eq!(
        scene.nodes.as_slice()[scene.roots.as_slice()[0]]
            .name
            .as_ref()
            .unwrap()
            .as_bytes(),
        b"first"
    );
}

#[test]
fn ordinary_binding_does_not_suppress_a_combined_world_root() {
    let mut objects = h::objects(false, false);
    objects[1] = Object::new(
        2,
        4,
        s::transform((0, 1), (0, 0), &[(0, 4), (0, 10)], false),
    );
    objects.extend([
        Object::new(
            9,
            1,
            s::game_object(b"combined", &[(0, 10), (0, 11), (0, 12)]),
        ),
        Object::new(10, 4, s::transform((0, 9), (0, 2), &[], false)),
        Object::new(11, 33, s::mesh_filter((0, 9), (0, 7))),
        Object::new(12, 23, s::mesh_renderer((0, 9), &[], (0, 2))),
    ]);
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = Profile::new(&m);
    p.static_batch = StaticBatch::KeepCombined;
    let scene = collect(&env, &g, &p, &m).unwrap();
    assert_eq!(
        scene
            .nodes
            .as_slice()
            .iter()
            .filter(|n| n.mesh.is_some())
            .count(),
        2
    );
    h::write(&scene);
}

#[test]
fn morph_endpoint_weight_uses_the_selected_frames_full_weight() {
    let mut objects = h::objects(true, false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[50.]),
    );
    let V::Struct(_, fields) = &mut objects[6].value else {
        panic!("mesh")
    };
    let (_, shape) = fields.iter_mut().find(|(k, _)| k == "m_Shapes").unwrap();
    s::set(
        shape,
        "fullWeights",
        unityrs_testkit::ngui::vector(V::Float(0.), vec![V::Float(25.), V::Float(50.)]),
    );
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert_eq!(
        scene
            .nodes
            .as_slice()
            .iter()
            .find(|n| n.mesh.is_some())
            .unwrap()
            .weights
            .as_slice(),
        [1.]
    );
}

#[test]
fn tangent_w_reflection_commutes_with_sign_normalisation_at_zero() {
    for source_w in [0f32, -0., f32::NAN, -f32::NAN] {
        let mut objects = h::objects(false, false);
        let V::Struct(_, fields) = &mut objects[6].value else {
            panic!("mesh")
        };
        let (_, V::Struct(_, vertex)) = fields
            .iter_mut()
            .find(|(k, _)| k == "m_VertexData")
            .unwrap()
        else {
            panic!("vertex")
        };
        let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap()
        else {
            panic!("data")
        };
        for row in 0..3 {
            data[row * 80 + 36..row * 80 + 40].copy_from_slice(&source_w.to_le_bytes());
        }
        let (env, g) = h::graph(&objects);
        let m = h::meter();
        let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
        let bytes = h::write(&scene);
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let doc: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
        let tangent = usize::try_from(
            doc["meshes"][0]["primitives"][0]["attributes"]["TANGENT"]
                .as_u64()
                .unwrap(),
        )
        .unwrap();
        let a = &doc["accessors"][tangent];
        let view = &doc["bufferViews"][usize::try_from(a["bufferView"].as_u64().unwrap()).unwrap()];
        let offset =
            28 + json_len + usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap() + 12;
        let w = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!(w, if source_w.is_sign_negative() { 1. } else { -1. });
    }
}

#[test]
fn unresolved_skin_joints_leave_a_reported_empty_node() {
    let mut objects = h::objects(true, false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 999)], &[25.]),
    );
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert!(scene.skins.is_empty());
    assert!(scene.meshes.is_empty());
    assert!(
        scene.nodes.as_slice()[1]
            .extras
            .as_slice()
            .iter()
            .any(|(k, _)| k.as_bytes() == b"unresolved_skin")
    );
    h::write(&scene);
}

#[test]
fn hidden_policy_marks_synthetic_drawable_roots_even_without_source_extras() {
    for (skin, batch) in [(true, false), (false, true)] {
        let mut objects = h::objects(skin, batch);
        s::set(&mut objects[2].value, "m_IsActive", V::Bool(false));
        let (env, g) = h::graph(&objects);
        let m = h::meter();
        let mut p = Profile::new(&m);
        p.inactive = Inactive::IncludeHidden;
        p.extras_naming = ExtrasNaming::None;
        let scene = collect(&env, &g, &p, &m).unwrap();
        let mesh = scene
            .nodes
            .as_slice()
            .iter()
            .find(|n| n.mesh.is_some())
            .unwrap();
        assert!(
            mesh.extras
                .as_slice()
                .iter()
                .any(|(k, v)| k.as_bytes() == b"hidden" && v.as_bytes() == b"true")
        );
        h::write(&scene);
    }
}

#[test]
fn morph_only_renderer_ignores_an_unused_missing_root_bone() {
    let mut objects = h::objects(true, false);
    let mut renderer = s::skinned_mesh_renderer((0, 3), (0, 7), &[], &[50.]);
    s::set(
        &mut renderer,
        "m_RootBone",
        unityrs_testkit::ngui::pptr("Transform", (0, 999)),
    );
    objects[5] = Object::new(6, 137, renderer);
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert!(scene.skins.is_empty());
    h::write(&scene);
}

#[test]
fn combined_root_is_visible_if_any_selected_instance_is_active() {
    let mut objects = h::objects(false, true);
    s::set(&mut objects[2].value, "m_IsActive", V::Bool(false));
    objects[1] = Object::new(
        2,
        4,
        s::transform((0, 1), (0, 0), &[(0, 4), (0, 10)], false),
    );
    objects.extend([
        Object::new(
            9,
            1,
            s::game_object(b"active", &[(0, 10), (0, 11), (0, 12)]),
        ),
        Object::new(10, 4, s::transform((0, 9), (0, 2), &[], false)),
        Object::new(11, 33, s::mesh_filter((0, 9), (0, 7))),
        Object::new(12, 23, s::mesh_renderer((0, 9), &[], (0, 1))),
    ]);
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = Profile::new(&m);
    p.static_batch = StaticBatch::KeepCombined;
    p.inactive = Inactive::IncludeHidden;
    let scene = collect(&env, &g, &p, &m).unwrap();
    let node = scene
        .nodes
        .as_slice()
        .iter()
        .find(|n| n.mesh.is_some())
        .unwrap();
    assert!(
        node.extras
            .as_slice()
            .iter()
            .all(|(key, _)| key.as_bytes() != b"hidden")
    );
}
