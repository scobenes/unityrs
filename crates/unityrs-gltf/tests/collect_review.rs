mod collect_support;
use collect_support as h;
use unityrs_gltf::*;

#[test]
fn skin_uses_common_ancestor_and_wraps_only_cross_root_joints() {
    for cross_root in [false, true] {
        let (env, graph) = h::graph(&h::hierarchy_objects(cross_root));
        let meter = h::meter();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        let skin = &scene.skins.as_slice()[0];
        let skeleton = skin.skeleton.unwrap();
        let synthetic: Vec<_> = scene
            .nodes
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.extras.as_slice().iter().any(|(k, v)| {
                    k.as_bytes() == b"unityrs_synthetic_root" && v.as_bytes() == b"true"
                })
            })
            .collect();
        assert_eq!(synthetic.len(), usize::from(cross_root));
        if cross_root {
            assert_eq!(synthetic[0].0, skeleton);
            assert_eq!(synthetic[0].1.children.len(), 2);
        } else {
            assert_eq!(
                scene.nodes.as_slice()[skeleton]
                    .name
                    .as_ref()
                    .unwrap()
                    .as_bytes(),
                b"hips"
            );
        }
        let (mesh_id, mesh) = scene
            .nodes
            .as_slice()
            .iter()
            .enumerate()
            .find(|(_, n)| n.skin.is_some())
            .unwrap();
        assert!(scene.roots.as_slice().contains(&mesh_id));
        assert_eq!(mesh.translation, [0.; 3]);
        // Identity binds, unit weights: vertex 1 follows spine (A) or prop (B).
        let mut positions = [[-1., 0., 0.], [0., 1., 0.], [0., 0., 0.]];
        for (world, joint_index) in positions.iter_mut().zip([0, 1, 0]) {
            let mut node = Some(skin.joints.as_slice()[joint_index]);
            while let Some(id) = node {
                for (v, t) in world.iter_mut().zip(scene.nodes.as_slice()[id].translation) {
                    *v += t;
                }
                node = scene
                    .nodes
                    .as_slice()
                    .iter()
                    .position(|n| n.children.as_slice().contains(&id));
            }
        }
        assert_eq!(
            positions,
            [
                [-2., 1., 0.],
                if cross_root {
                    [0., 1., 0.]
                } else {
                    [-1., 3., 0.]
                },
                [-1., 1., 0.]
            ]
        );
        h::validator_clean(&scene);
    }
}

use unityrs_testkit::{
    ngui::{Object, V, vector},
    scene3d::{self as s, mesh as m},
};
fn document(scene: &Scene) -> serde_json::Value {
    h::validator_clean(scene);
    let bytes = h::write(scene);
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + len]).unwrap()
}
fn issue(scene: &Scene, reason: &str, count: u64) {
    let meter = h::meter();
    let mut bytes = Vec::new();
    let report = write_glb(scene, &Options::default(), &meter, &mut bytes).unwrap();
    assert!(
        report
            .collection_issues
            .as_slice()
            .iter()
            .any(|i| i.reason == reason && i.count == count)
    );
    let doc = document(scene);
    assert!(
        doc["asset"]["extras"]["collection_issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["reason"] == reason && i["count"] == count),
        "{doc}"
    );
}
#[test]
fn blend_weights_are_padded_or_truncated_per_renderer() {
    for (weights, want, reason) in [
        (vec![], vec![0.], "blend_weights_padded"),
        (vec![25., 50.], vec![0.25], "blend_weights_truncated"),
    ] {
        let mut objects = h::objects(true, false);
        objects[5] = Object::new(
            6,
            137,
            s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &weights),
        );
        let (env, graph) = h::graph(&objects);
        let meter = h::meter();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        let node = scene
            .nodes
            .as_slice()
            .iter()
            .find(|n| n.mesh.is_some())
            .unwrap();
        assert_eq!(node.weights.as_slice(), want);
        issue(&scene, reason, 1);
    }
}
#[test]
fn duplicate_bones_remap_only_their_skin_and_merge_influences() {
    let mut objects = h::hierarchy_objects(false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 8), (0, 10), (0, 8)], &[25.]),
    );
    let identity = m::matrix([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(identity.clone(), vec![identity.clone(); 3]),
    );
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
    let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap() else {
        panic!("data")
    };
    for row in 0..3 {
        data[row * 80 + 64..row * 80 + 68].copy_from_slice(&0u32.to_le_bytes());
        data[row * 80 + 48..row * 80 + 52].copy_from_slice(&0.25f32.to_le_bytes());
        data[row * 80 + 52..row * 80 + 56].copy_from_slice(&0.75f32.to_le_bytes());
        data[row * 80 + 68..row * 80 + 72].copy_from_slice(&2u32.to_le_bytes());
    }
    objects[2] = Object::new(3, 1, s::game_object(b"body", &[(0, 4), (0, 6)]));
    objects[11] = Object::new(13, 1, s::game_object(b"prop", &[(0, 12), (0, 14)]));
    objects.push(Object::new(
        14,
        137,
        s::skinned_mesh_renderer((0, 13), (0, 7), &[(0, 8), (0, 10), (0, 12)], &[25.]),
    ));
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
    assert_eq!(scene.skins.as_slice()[0].joints.len(), 2);
    assert_eq!(scene.skins.as_slice()[1].joints.len(), 3);
    let nodes: Vec<_> = scene
        .nodes
        .as_slice()
        .iter()
        .filter(|n| n.skin.is_some())
        .collect();
    let attrs = |n: usize| {
        scene.meshes.as_slice()[nodes[n].mesh.unwrap()]
            .primitives
            .as_slice()[0]
            .attributes
            .as_slice()
    };
    let get = |n, kind| attrs(n).iter().find(|(k, _)| *k == kind).unwrap().1;
    assert_ne!(get(0, Attribute::Joints), get(1, Attribute::Joints));
    let Data::U16(joints) = &scene.accessors.as_slice()[get(0, Attribute::Joints)].data else {
        panic!("joints")
    };
    assert_eq!(joints.as_slice(), [0; 12]);
    assert_eq!(
        h::floats(&scene, get(0, Attribute::Weights)),
        [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.]
    );
    assert_eq!(
        h::floats(&scene, get(1, Attribute::Weights))[..4],
        [0.25, 0.75, 0., 0.]
    );
    issue(&scene, "duplicate_joints", 1);
}
#[test]
fn bones_without_vertex_skin_attributes_export_at_renderer_transform() {
    let mut objects = h::objects(true, false);
    objects[6] = m::mesh(
        7,
        b"plain",
        &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        &[0, 1, 2],
        m::MeshVersion::V2022,
    );
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
    assert!(scene.skins.is_empty());
    let node = scene
        .nodes
        .as_slice()
        .iter()
        .find(|n| n.mesh.is_some())
        .unwrap();
    assert_eq!(node.translation, [-2., 0., 0.]);
    issue(&scene, "missing_skin_attributes", 1);
}
#[test]
fn bind_pose_mismatch_degrades_renderer_and_keeps_other_geometry() {
    let mut objects = h::objects(true, false);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(m::matrix([0.; 16]), vec![]),
    );
    objects[0] = Object::new(
        1,
        1,
        s::game_object(b"healthy", &[(0, 2), (0, 20), (0, 21)]),
    );
    objects.extend([
        Object::new(20, 33, s::mesh_filter((0, 1), (0, 7))),
        Object::new(21, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
    ]);
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
    assert!(scene.skins.is_empty());
    assert_eq!(
        scene
            .nodes
            .as_slice()
            .iter()
            .filter(|n| n.mesh.is_some())
            .count(),
        1
    );
    assert_eq!(scene.nodes.as_slice()[1].mesh, None);
    issue(&scene, "skin_bind_pose_count", 1);
}

fn batches(legacy: bool) -> Vec<Object> {
    let mut objects = h::objects(false, true);
    objects[1] = Object::new(
        2,
        4,
        s::transform((0, 1), (0, 0), &[(0, 4), (0, 10)], false),
    );
    objects[0] = Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 8)]));
    objects.extend([
        Object::new(
            8,
            205,
            s::lod_group((0, 1), &[(0.8, vec![(0, 6)]), (0.2, vec![(0, 12)])]),
        ),
        Object::new(9, 1, s::game_object(b"other", &[(0, 10), (0, 11), (0, 12)])),
        Object::new(10, 4, s::transform((0, 9), (0, 2), &[], false)),
        Object::new(11, 33, s::mesh_filter((0, 9), (0, 7))),
        Object::new(12, 23, s::mesh_renderer((0, 9), &[], (1, 1))),
    ]);
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[], (0, 1)));
    if legacy {
        for (index, go, subset) in [(5, 3, 0), (11, 9, 1)] {
            objects[index] = Object::new(
                if index == 5 { 6 } else { 12 },
                23,
                s::mesh_renderer((0, go), &[], (0, 0)),
            );
            s::set(
                &mut objects[index].value,
                "m_SubsetIndices",
                vector(V::UInt(0), vec![V::UInt(subset)]),
            );
        }
    }
    objects
}
#[test]
fn legacy_subset_batches_use_identity_roots_and_deduplicate_combined_draws() {
    let (env, graph) = h::graph(&batches(true));
    let meter = h::meter();
    let mut profile = Profile::new(&meter);
    profile.lod = Lod::All;
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    let draws: Vec<_> = scene
        .nodes
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, n)| n.mesh.is_some())
        .collect();
    assert_eq!(draws.len(), 2);
    for (id, node) in draws {
        assert_eq!(node.translation, [0.; 3]);
        assert!(scene.roots.as_slice().contains(&id));
    }
    assert!(scene.nodes.as_slice().iter().any(|n| {
        n.extras
            .as_slice()
            .iter()
            .any(|(k, _)| k.as_bytes() == b"static_batched")
    }));
    profile.static_batch = StaticBatch::KeepCombined;
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    assert_eq!(
        scene
            .nodes
            .as_slice()
            .iter()
            .filter(|n| n.mesh.is_some())
            .count(),
        1
    );
    assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 2);
    h::validator_clean(&scene);
}
#[test]
fn keep_combined_draws_only_the_union_of_selected_renderer_ranges() {
    for legacy in [false, true] {
        for exclusion in ["lod", "inactive", "layer", "disabled"] {
            let mut objects = batches(legacy);
            match exclusion {
                "inactive" => s::set(&mut objects[8].value, "m_IsActive", V::Bool(false)),
                "layer" => s::set(&mut objects[8].value, "m_Layer", V::UInt(7)),
                "disabled" => s::set(&mut objects[11].value, "m_Enabled", V::Bool(false)),
                _ => (),
            }
            let (env, graph) = h::graph(&objects);
            let meter = h::meter();
            let mut profile = Profile::new(&meter);
            profile.static_batch = StaticBatch::KeepCombined;
            if exclusion != "lod" {
                profile.lod = Lod::All;
            }
            if exclusion == "layer" {
                profile.layer_skips.push(7).unwrap();
            }
            let scene = collect(&env, &graph, &profile, &meter).unwrap();
            assert_eq!(
                scene
                    .nodes
                    .as_slice()
                    .iter()
                    .filter(|n| n.mesh.is_some())
                    .count(),
                1
            );
            assert_eq!(
                scene.meshes.as_slice()[0].primitives.len(),
                1,
                "{legacy} {exclusion}"
            );
            h::validator_clean(&scene);
        }
    }
}

#[test]
fn conflicting_duplicate_bind_poses_degrade_without_a_broken_skin() {
    let mut objects = h::objects(true, false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2), (0, 2)], &[25.]),
    );
    let identity = m::matrix([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(identity.clone(), vec![identity, m::matrix([0.; 16])]),
    );
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
    assert!(scene.skins.is_empty());
    assert!(scene.nodes.as_slice().iter().all(|n| n.mesh.is_none()));
    issue(&scene, "duplicate_joint_bind_conflict", 1);
}

#[test]
fn invalid_geometry_or_batch_range_does_not_fail_other_renderers() {
    for bad_geometry in [false, true] {
        let mut objects = h::objects(false, false);
        if bad_geometry {
            s::set(
                &mut objects[6].value,
                "m_SubMeshes",
                vector(
                    m::submesh(0, 0, 0, 0, 0, 0),
                    vec![m::submesh(0, 3, 0, 999, 0, 3)],
                ),
            );
        } else {
            objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[], (3, 1)));
        }
        objects[0] = Object::new(
            1,
            1,
            s::game_object(b"healthy", &[(0, 2), (0, 20), (0, 21)]),
        );
        objects.extend([
            Object::new(20, 33, s::mesh_filter((0, 1), (0, 70))),
            Object::new(21, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
            m::mesh(
                70,
                b"healthy",
                &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
                &[0, 1, 2],
                m::MeshVersion::V2022,
            ),
        ]);
        let (env, graph) = h::graph(&objects);
        let meter = h::meter();
        let mut profile = Profile::new(&meter);
        profile.static_batch = StaticBatch::KeepCombined;
        let scene = collect(&env, &graph, &profile, &meter).unwrap();
        assert_eq!(
            scene
                .nodes
                .as_slice()
                .iter()
                .filter(|n| n.mesh.is_some())
                .count(),
            1
        );
        assert!(
            scene.nodes.as_slice()[1]
                .extras
                .as_slice()
                .iter()
                .any(|(k, _)| k.as_bytes() == b"unresolved_renderer")
        );
        assert_eq!(scene.collection_issues.len(), 1);
        h::validator_clean(&scene);
    }
}

#[test]
fn skin_influence_repairs_and_range_failures_are_reported_before_writing() {
    for duplicate_influence in [false, true] {
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
        let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap()
        else {
            panic!("data")
        };
        if duplicate_influence {
            data[52..56].copy_from_slice(&1f32.to_le_bytes());
        } else {
            data[64..68].copy_from_slice(&9u32.to_le_bytes());
        }
        let (env, graph) = h::graph(&objects);
        let meter = h::meter();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        if duplicate_influence {
            assert_eq!(scene.skins.len(), 1);
            assert!(scene.nodes.as_slice().iter().any(|n| n.mesh.is_some()));
        } else {
            assert!(scene.skins.is_empty());
            assert!(scene.nodes.as_slice().iter().all(|n| n.mesh.is_none()));
        }
        issue(
            &scene,
            if duplicate_influence {
                "duplicate_skin_influence"
            } else {
                "skin_joint_palette_range"
            },
            1,
        );
    }
}
#[test]
fn merging_duplicate_joint_weights_preserves_large_finite_weight_ratios() {
    let mut objects = h::repair_objects();
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
    let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap() else {
        panic!("data")
    };
    for (slot, joint) in [0u32, 2, 1].into_iter().enumerate() {
        data[48 + slot * 4..52 + slot * 4].copy_from_slice(&f32::MAX.to_le_bytes());
        data[64 + slot * 4..68 + slot * 4].copy_from_slice(&joint.to_le_bytes());
    }
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
    let weights = scene.meshes.as_slice()[0].primitives.as_slice()[0]
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == Attribute::Weights)
        .unwrap()
        .1;
    let row = h::floats(&scene, weights);
    assert!((row[0] - 2. / 3.).abs() < 1e-7);
    assert!((row[1] - 1. / 3.).abs() < 1e-7);
    h::validator_clean(&scene);
}

#[test]
fn nonfinite_blend_weights_and_invalid_bind_matrices_degrade_the_renderer() {
    for bad_blend in [false, true] {
        let mut objects = h::objects(true, false);
        if bad_blend {
            objects[5] = Object::new(
                6,
                137,
                s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[f32::NAN]),
            );
        } else {
            s::set(
                &mut objects[6].value,
                "m_BindPose",
                vector(m::matrix([0.; 16]), vec![m::matrix([0.; 16])]),
            );
        }
        let (env, graph) = h::graph(&objects);
        let meter = h::meter();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        assert!(scene.nodes.as_slice().iter().all(|n| n.mesh.is_none()));
        issue(
            &scene,
            if bad_blend {
                "invalid_blend_weights"
            } else {
                "invalid_bind_matrix"
            },
            1,
        );
    }
}

#[test]
fn raw_duplicate_palette_keeps_signed_weights() {
    let mut objects = h::repair_objects();
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
    let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap() else {
        panic!("data")
    };
    data[48..52].copy_from_slice(&(-0.5f32).to_le_bytes());
    data[52..56].copy_from_slice(&1.5f32.to_le_bytes());
    data[68..72].copy_from_slice(&1u32.to_le_bytes());
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let mut profile = Profile::new(&meter);
    profile.normalise = false;
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    let weights = scene.meshes.as_slice()[0].primitives.as_slice()[0]
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == Attribute::Weights)
        .unwrap()
        .1;
    assert_eq!(&h::floats(&scene, weights)[..4], [-0.5, 1.5, 0., 0.]);
    write_glb(&scene, &profile.options(), &meter, &mut Vec::new()).unwrap();
}
#[test]
fn nonfinite_uv_or_morph_data_degrades_only_its_renderer() {
    for morph in [false, true] {
        let mut objects = h::objects(false, false);
        if morph {
            s::set(
                &mut objects[6].value,
                "m_Shapes",
                m::blend_shapes(
                    vec![m::blend_vertex(0, [f32::NAN, 0., 0.], [0.; 3], [0.; 3])],
                    vec![m::blend_frame(0, 1, false, false)],
                    vec![m::blend_channel(b"bad", 0, 0, 1)],
                    vec![100.],
                ),
            );
        } else {
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
            data[40..44].copy_from_slice(&f32::NAN.to_le_bytes());
        }
        let (env, graph) = h::graph(&objects);
        let meter = h::meter();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        assert!(scene.nodes.as_slice().iter().all(|n| n.mesh.is_none()));
        assert_eq!(scene.collection_issues.len(), 1);
        h::validator_clean(&scene);
    }
}
