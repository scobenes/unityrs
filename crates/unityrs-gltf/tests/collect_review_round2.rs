mod collect_support;
use collect_support as h;
use unityrs_gltf::*;
use unityrs_testkit::{
    ngui::{Object, V, pptr, vector},
    scene3d::{self as s, mesh as m},
};

fn document(scene: &Scene) -> (serde_json::Value, Report) {
    h::validator_clean(scene);
    let mut bytes = Vec::new();
    let report = write_glb(scene, &Options::default(), &h::meter(), &mut bytes).unwrap();
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[20..20 + len]).unwrap(),
        report,
    )
}
fn collected(objects: &[Object]) -> Scene {
    let (env, graph) = h::graph(objects);
    let meter = h::meter();
    collect(&env, &graph, &Profile::new(&meter), &meter).unwrap()
}
fn influences(objects: &mut [Object], joints: [u32; 4], weights: [f32; 4]) {
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
        for slot in 0..4 {
            let offset = row * 80 + slot * 4;
            data[offset + 48..offset + 52].copy_from_slice(&weights[slot].to_le_bytes());
            data[offset + 64..offset + 68].copy_from_slice(&joints[slot].to_le_bytes());
        }
    }
}
fn attribute(scene: &Scene, kind: Attribute) -> usize {
    scene.meshes.as_slice()[0].primitives.as_slice()[0]
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == kind)
        .unwrap()
        .1
}
fn issue(doc: &serde_json::Value, reason: &str, count: u64) {
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
fn repeated_repairs_sum_node_extras_and_keep_each_report_record() {
    let mut objects = h::objects(true, false);
    objects[2] = Object::new(3, 1, s::game_object(b"child", &[(0, 4), (0, 6), (0, 8)]));
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[]),
    );
    objects.push(Object::new(
        8,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[]),
    ));
    let scene = collected(&objects);
    let (doc, report) = document(&scene);
    let issues: Vec<_> = report
        .collection_issues
        .as_slice()
        .iter()
        .filter(|i| i.reason == "blend_weights_padded")
        .collect();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].node, issues[1].node);
    assert!(issues.iter().all(|i| i.count == 1));
    assert_eq!(
        doc["nodes"][issues[0].node]["extras"]["blend_weights_padded"],
        "2"
    );
    assert_eq!(
        doc["asset"]["extras"]["collection_issues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let extras = scene.nodes.as_slice()[issues[0].node].extras.as_slice();
    assert!(
        extras
            .windows(2)
            .all(|w| w[0].0.as_bytes() < w[1].0.as_bytes())
    );
    assert_eq!(h::write(&scene), h::write(&collected(&objects)));
}

#[test]
fn unusable_optional_roots_fall_back_without_losing_resolved_joints() {
    for target in [999, 7] {
        // Unresolved pointer, then a resolved object outside the graph.
        let mut objects = h::hierarchy_objects(false);
        s::set(
            &mut objects[5].value,
            "m_RootBone",
            pptr("Transform", (0, target)),
        );
        let scene = collected(&objects);
        assert_eq!(scene.skins.len(), 1);
        let skin = &scene.skins.as_slice()[0];
        assert_eq!(skin.joints.len(), 2);
        assert_eq!(
            scene.nodes.as_slice()[skin.skeleton.unwrap()]
                .name
                .as_ref()
                .unwrap()
                .as_bytes(),
            b"hips"
        );
        assert_eq!(
            scene
                .nodes
                .as_slice()
                .iter()
                .filter(|n| n.skin.is_some())
                .count(),
            1
        );
        let (doc, report) = document(&scene);
        assert!(
            report
                .collection_issues
                .as_slice()
                .iter()
                .any(|i| i.reason == "skin_root_fallback" && i.count == 1)
        );
        issue(&doc, "skin_root_fallback", 1);
    }
}

#[test]
fn palette_weight_repairs_survive_in_report_and_asset_extras_once_per_accessor() {
    for (weights, changes, zeros) in [
        ([2., 2., 0., 0.], 6, 0),
        ([-1., 2., 0., 0.], 6, 0),
        ([f32::NAN, 2., 0., 0.], 6, 0),
        ([-1., 0., 0., 0.], 3, 3),
    ] {
        let mut objects = h::repair_objects();
        // Avoid unrelated blend padding; the duplicate palette still runs.
        s::set(
            &mut objects[5].value,
            "m_BlendShapeWeights",
            vector(V::Float(0.), vec![V::Float(25.)]),
        );
        influences(&mut objects, [0, 1, 0, 0], weights);
        let scene = collected(&objects);
        let (doc, report) = document(&scene);
        assert_eq!(report.weight_components, changes, "{weights:?}");
        assert_eq!(report.zero_weight_rows, zeros);
        assert_eq!(doc["asset"]["extras"]["weight_components"], changes);
        assert_eq!(doc["asset"]["extras"]["zero_weight_rows"], zeros);
        // Two primitives share this accessor; its changes are counted once.
        assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 2);
    }
}

#[test]
fn repeated_vertex_influences_merge_and_draw_fully_on_spine() {
    let mut objects = h::hierarchy_objects(false);
    influences(&mut objects, [1, 1, 0, 0], [0.5, 0.5, 0., 0.]);
    let scene = collected(&objects);
    assert_eq!(scene.skins.len(), 1);
    assert_eq!(scene.skins.as_slice()[0].joints.len(), 2);
    let Data::U16(joints) = &scene.accessors.as_slice()[attribute(&scene, Attribute::Joints)].data
    else {
        panic!("joints")
    };
    assert_eq!(joints.as_slice(), [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(
        h::floats(&scene, attribute(&scene, Attribute::Weights)),
        [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.]
    );
    let (doc, _) = document(&scene);
    issue(&doc, "duplicate_skin_influence", 3);
}

#[test]
fn bind_pose_affine_noise_is_snapped_and_counted() {
    for (row, repaired) in [
        ([0., 0., 0., 0.999_999_94], true),
        ([0.000_000_5, -0.000_000_5, 0.000_001, 1.], true),
        ([0.000_002, 0., 0., 1.], false),
        ([0., 0., 0., f32::NAN], false),
    ] {
        let mut objects = h::objects(true, false);
        let matrix = m::matrix([
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., row[0], row[1], row[2], row[3],
        ]);
        s::set(
            &mut objects[6].value,
            "m_BindPose",
            vector(matrix.clone(), vec![matrix]),
        );
        let scene = collected(&objects);
        assert_eq!(scene.skins.len(), usize::from(repaired));
        let (doc, _) = document(&scene);
        if repaired {
            let values = h::floats(
                &scene,
                scene.skins.as_slice()[0].inverse_bind_matrices.unwrap(),
            );
            assert_eq!(
                [values[3], values[7], values[11], values[15]],
                [0., 0., 0., 1.]
            );
            issue(&doc, "bind_pose_rows_snapped", 1);
        } else {
            issue(&doc, "invalid_bind_matrix", 1);
        }
    }
}

#[test]
fn duplicate_bind_poses_compare_their_repaired_affine_rows() {
    let mut objects = h::objects(true, false);
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2), (0, 2)], &[25.]),
    );
    let identity = m::matrix([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    let noisy = m::matrix([
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        0.,
        0.,
        0.,
        0.999_999_94,
    ]);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(identity.clone(), vec![identity, noisy]),
    );
    let scene = collected(&objects);
    assert_eq!(scene.skins.len(), 1);
    assert_eq!(scene.skins.as_slice()[0].joints.len(), 1);
    let (doc, _) = document(&scene);
    issue(&doc, "bind_pose_rows_snapped", 1);
    issue(&doc, "duplicate_joints", 1);
}

#[test]
fn influence_merging_ignores_indices_with_weights_that_round_to_zero() {
    for duplicate_palette in [false, true] {
        let mut objects = if duplicate_palette {
            h::repair_objects()
        } else {
            h::hierarchy_objects(false)
        };
        influences(
            &mut objects,
            [1, 1, 999, 0],
            [1., 1., f32::from_bits(1), 0.],
        );
        let scene = collected(&objects);
        assert_eq!(scene.skins.len(), 1);
        assert_eq!(
            &h::floats(&scene, attribute(&scene, Attribute::Weights))[..4],
            [1., 0., 0., 0.]
        );
        let (doc, _) = document(&scene);
        issue(&doc, "duplicate_skin_influence", 3);
    }
}
