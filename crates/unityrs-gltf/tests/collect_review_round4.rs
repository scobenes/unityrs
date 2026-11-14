mod collect_support;
use collect_support as h;
use unityrs_core::{
    Bytes, RawStr,
    env::{EnvOptions, Environment, Source},
    files::File,
};
use unityrs_gltf::*;
use unityrs_scene::{Graph, GraphOptions, graph_from_scene_file};
use unityrs_testkit::{
    ngui::{Object, V, st, vector},
    scene3d::{self as s, mesh as m},
};

fn builtin_graph(objects: &[Object]) -> (Environment, Graph) {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(
                s::file(
                    s::Layout::Unity2022,
                    &[
                        "Library/unity default resources",
                        "Resources/unity_builtin_extra",
                    ],
                    objects,
                )
                .unwrap(),
            ),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &h::meter()).unwrap();
    (env, graph)
}

#[test]
fn resolved_builtin_batches_union_selected_ranges_by_source_identity() {
    for legacy in [false, true] {
        for parent_enabled in [false, true] {
            for different_source in [false, true] {
                let mut objects = h::objects(false, true);
                objects[0] =
                    Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 10), (0, 11)]));
                objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
                let mut parent = s::mesh_renderer((0, 1), &[], (0, 1));
                s::set(&mut parent, "m_Enabled", V::Bool(parent_enabled));
                if legacy {
                    for (renderer, subset) in [(&mut objects[5].value, 1), (&mut parent, 0)] {
                        s::set(
                            renderer,
                            "m_StaticBatchInfo",
                            st(
                                "StaticBatchInfo",
                                vec![("firstSubMesh", V::UInt(0)), ("subMeshCount", V::UInt(0))],
                            ),
                        );
                        s::set(
                            renderer,
                            "m_SubsetIndices",
                            vector(V::UInt(0), vec![V::UInt(subset)]),
                        );
                    }
                }
                objects.extend([
                    Object::new(
                        10,
                        33,
                        s::mesh_filter((0, 1), (if different_source { 2 } else { 1 }, 10210)),
                    ),
                    Object::new(11, 23, parent),
                ]);
                let (env, graph) = builtin_graph(&objects);
                let mut mesh = m::mesh(
                    10210,
                    b"resolved",
                    &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                    &[0, 1, 2, 0, 1, 2],
                    m::MeshVersion::V2022,
                );
                s::set(
                    &mut mesh.value,
                    "m_SubMeshes",
                    vector(
                        m::submesh(0, 0, 0, 0, 0, 0),
                        vec![m::submesh(0, 3, 0, 0, 0, 3), m::submesh(12, 3, 0, 0, 0, 3)],
                    ),
                );
                for name in [
                    b"unity default resources".as_slice(),
                    b"unity_builtin_extra",
                ] {
                    env.load(Source::Bytes {
                        name: RawStr::from_static(name),
                        data: Bytes::from(
                            s::file(s::Layout::Unity2022, &[], &[mesh.clone()]).unwrap(),
                        ),
                    })
                    .unwrap();
                }
                let meter = h::meter();
                let mut profile = Profile::new(&meter);
                profile.builtin = Builtin::Resolve;
                profile.static_batch = StaticBatch::KeepCombined;
                let scene = collect(&env, &graph, &profile, &meter).unwrap();
                let draws = usize::from(parent_enabled && different_source) + 1;
                assert_eq!(scene.meshes.len(), draws);
                assert_eq!(
                    scene
                        .nodes
                        .as_slice()
                        .iter()
                        .filter(|n| n.mesh.is_some())
                        .count(),
                    draws
                );
                assert_eq!(
                    scene
                        .meshes
                        .as_slice()
                        .iter()
                        .map(|m| m.primitives.len())
                        .sum::<usize>(),
                    usize::from(parent_enabled) + 1
                );
                for node in scene.nodes.as_slice().iter().filter(|n| n.mesh.is_some()) {
                    assert_eq!(node.translation, [0.; 3]);
                    assert_eq!(node.rotation, [0., 0., 0., 1.]);
                    assert_eq!(node.scale, [1.; 3]);
                }
                assert!(scene.collection_issues.is_empty());
                h::validator_clean(&scene);
            }
        }
    }
}

#[test]
fn builtin_placeholders_report_each_renderer_in_report_and_asset_extras() {
    let mut objects = h::objects(false, false);
    objects[2] = Object::new(
        3,
        1,
        s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 10), (0, 11)]),
    );
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    objects.extend([
        Object::new(10, 23, s::mesh_renderer((0, 3), &[], (0, 0))),
        Object::new(
            11,
            137,
            s::skinned_mesh_renderer((0, 3), (1, 10207), &[], &[]),
        ),
    ]);
    let (env, graph) = builtin_graph(&objects);
    let meter = h::meter();
    let profile = Profile::new(&meter);
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    assert!(scene.meshes.is_empty());
    assert_eq!(scene.collection_issues.len(), 3);
    let mut bytes = Vec::new();
    let report = write_glb(&scene, &profile.options(), &meter, &mut bytes).unwrap();
    assert_eq!(report.collection_issues.len(), 3);
    for issue in report.collection_issues.as_slice() {
        assert_eq!(issue.node, 1);
        assert_eq!(issue.reason, "builtin_placeholder");
        assert_eq!(issue.count, 1);
    }
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
    assert_eq!(
        json["asset"]["extras"]["collection_issues"],
        serde_json::json!([
            {"node": 1, "reason": "builtin_placeholder", "count": 1},
            {"node": 1, "reason": "builtin_placeholder", "count": 1},
            {"node": 1, "reason": "builtin_placeholder", "count": 1}
        ])
    );
    assert_eq!(json["nodes"][1]["extras"]["unity_builtin"], "10207;10210");
    h::validator_clean(&scene);
}
