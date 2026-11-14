mod collect_support;
use collect_support as h;
use std::sync::Arc;
use unityrs_core::{
    Bytes, Error, LimitError, LimitKind, RawStr,
    env::{CabResolver, EnvOptions, Environment, Source},
    files::File,
};
use unityrs_gltf::*;
use unityrs_scene::{Graph, GraphOptions, graph_from_scene_file};
use unityrs_testkit::{
    ngui::{Object, V, st, vector},
    scene3d::{self as s, mesh as m},
};

fn field<'a>(value: &'a mut V, name: &str) -> &'a mut V {
    let V::Struct(_, fields) = value else {
        panic!("struct")
    };
    &mut fields.iter_mut().find(|(key, _)| key == name).unwrap().1
}
fn plain_mesh(id: i64) -> Object {
    m::mesh(
        id,
        b"healthy",
        &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        &[0, 1, 2],
        m::MeshVersion::V2022,
    )
}
fn extra<'a>(scene: &'a Scene, node: usize, key: &str) -> &'a [u8] {
    scene.nodes.as_slice()[node]
        .extras
        .as_slice()
        .iter()
        .find(|(k, _)| k.as_bytes() == key.as_bytes())
        .unwrap()
        .1
        .as_bytes()
}
fn document(scene: &Scene, profile: &Profile) -> (serde_json::Value, Report) {
    let mut bytes = Vec::new();
    let report = write_glb(scene, &profile.options(), &h::meter(), &mut bytes).unwrap();
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[20..20 + length]).unwrap(),
        report,
    )
}
fn add_healthy(objects: &mut Vec<Object>) {
    objects[0] = Object::new(
        1,
        1,
        s::game_object(b"healthy", &[(0, 2), (0, 20), (0, 21)]),
    );
    objects.extend([
        Object::new(20, 33, s::mesh_filter((0, 1), (0, 70))),
        Object::new(21, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
        plain_mesh(70),
    ]);
}

fn degraded_objects(skin_failure: bool, different: bool, reverse: bool) -> Vec<Object> {
    let mut objects = h::objects(true, false);
    let first = if skin_failure {
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 999)], &[25.])
    } else {
        s::skinned_mesh_renderer((0, 3), (0, 999), &[], &[25.])
    };
    let second = if different {
        if skin_failure {
            s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[f32::NAN])
        } else {
            s::skinned_mesh_renderer((0, 3), (0, 0), &[], &[])
        }
    } else {
        first.clone()
    };
    objects[2] = Object::new(
        3,
        1,
        s::game_object(b"child", &[(0, 4), (0, 6), (0, 10), (0, 11)]),
    );
    objects[5] = Object::new(
        6,
        137,
        if reverse {
            second.clone()
        } else {
            first.clone()
        },
    );
    objects.extend([
        Object::new(10, 137, if reverse { first } else { second }),
        Object::new(
            11,
            137,
            s::skinned_mesh_renderer((0, 3), (0, 7), &[], &[75.]),
        ),
    ]);
    objects
}

#[test]
fn repeated_degradations_aggregate_markers_and_keep_every_report_record() {
    for skin_failure in [false, true] {
        for different in [false, true] {
            for reverse in [false, true] {
                let objects = degraded_objects(skin_failure, different, reverse);
                let (env, graph) = h::graph(&objects);
                let meter = h::meter();
                let profile = Profile::new(&meter);
                let scene = collect(&env, &graph, &profile, &meter).unwrap();
                let reason = if skin_failure {
                    "joint unresolved or outside graph"
                } else {
                    "mesh_unresolved"
                };
                let other = if skin_failure {
                    "invalid_blend_weights"
                } else {
                    "missing_mesh"
                };
                let expected = if different {
                    let mut reasons = [reason, other];
                    reasons.sort_unstable();
                    reasons.join(";")
                } else {
                    reason.into()
                };
                assert_eq!(
                    extra(
                        &scene,
                        1,
                        if skin_failure {
                            "unresolved_skin"
                        } else {
                            "unresolved_renderer"
                        }
                    ),
                    expected.as_bytes()
                );
                assert_eq!(
                    extra(&scene, 1, reason),
                    if different { b"1" } else { b"2" }
                );
                assert_eq!(scene.collection_issues.len(), 2);
                assert_eq!(
                    scene
                        .nodes
                        .as_slice()
                        .iter()
                        .filter(|n| n.mesh.is_some())
                        .count(),
                    1
                );
                assert_eq!(scene.nodes.as_slice()[1].weights.as_slice(), [0.75]);
                let (json, report) = document(&scene, &profile);
                assert_eq!(report.collection_issues.len(), 2);
                assert_eq!(
                    json["asset"]["extras"]["collection_issues"]
                        .as_array()
                        .unwrap()
                        .len(),
                    2
                );
                assert_eq!(
                    report.collection_issues.as_slice()[0].reason,
                    if reverse && different { other } else { reason }
                );
                h::validator_clean(&scene);
            }
        }
    }
}

#[test]
fn multiple_morph_only_and_downgraded_renderers_keep_transforms_and_weights() {
    for downgrade in [false, true] {
        for normalise in [false, true] {
            let mut objects = h::objects(true, false);
            if downgrade {
                let vertex = field(&mut plain_mesh(7).value, "m_VertexData").clone();
                s::set(&mut objects[6].value, "m_VertexData", vertex);
            }
            s::set(
                &mut objects[3].value,
                "m_LocalRotation",
                s::xyz("Quaternionf", &[0., 0., 0.6, 0.8]),
            );
            s::set(
                &mut objects[3].value,
                "m_LocalScale",
                s::xyz("Vector3f", &[2., 3., 4.]),
            );
            objects[2] = Object::new(
                3,
                1,
                s::game_object(b"child", &[(0, 4), (0, 6), (0, 10), (0, 11)]),
            );
            for (id, weight) in [(6, 25.), (10, 50.), (11, 75.)] {
                let renderer = Object::new(
                    id,
                    137,
                    s::skinned_mesh_renderer(
                        (0, 3),
                        (0, 7),
                        if downgrade { &[(0, 2)] } else { &[] },
                        &[weight],
                    ),
                );
                if id == 6 {
                    objects[5] = renderer;
                } else {
                    objects.push(renderer);
                }
            }
            let (env, graph) = h::graph(&objects);
            let meter = h::meter();
            let mut profile = Profile::new(&meter);
            profile.normalise = normalise;
            let scene = collect(&env, &graph, &profile, &meter).unwrap();
            assert!(scene.skins.is_empty());
            assert_eq!(scene.roots.as_slice(), [0]);
            let original = &scene.nodes.as_slice()[1];
            assert_eq!(original.translation, [-2., 0., 0.]);
            assert_eq!(original.scale, [2., 3., 4.]);
            assert_eq!(
                original.rotation,
                [0., 0., -f64::from(0.6f32), f64::from(0.8f32)]
            );
            assert_eq!(original.children.len(), 2);
            let attachments = [
                1,
                original.children.as_slice()[0],
                original.children.as_slice()[1],
            ];
            for (index, id) in attachments.into_iter().enumerate() {
                let node = &scene.nodes.as_slice()[id];
                assert_eq!(node.mesh, original.mesh);
                assert_eq!(
                    node.weights.as_slice(),
                    [f64::from(u32::try_from(index + 1).unwrap()) * 0.25]
                );
                if index > 0 {
                    assert_eq!(node.translation, [0.; 3]);
                    assert_eq!(node.rotation, [0., 0., 0., 1.]);
                    assert_eq!(node.scale, [1.; 3]);
                }
            }
            let (_, report) = document(&scene, &profile);
            assert_eq!(
                report.collection_issues.len(),
                if downgrade { 3 } else { 0 }
            );
            if downgrade {
                assert_eq!(extra(&scene, 1, "missing_skin_attributes"), b"3");
            }
            h::validator_clean(&scene);
        }
    }
}

#[test]
fn compressed_optional_attribute_count_mismatches_degrade_only_the_bad_renderer() {
    for items in [4, 16] {
        let attribute = "m_FloatColors";
        for normalise in [false, true] {
            let mut objects = h::objects(false, false);
            let mut compressed = m::compressed();
            // One or four decoded rows next to three POSITION rows. MeshHandler retains
            // these compressed rows as-is; collection must check glTF counts.
            s::set(
                &mut compressed,
                attribute,
                st(
                    "PackedBitVector",
                    vec![
                        ("m_NumItems", V::UInt(items)),
                        ("m_Range", V::Float(0.)),
                        ("m_Start", V::Float(0.)),
                        ("m_Data", vector(V::UInt8(0), vec![])),
                        ("m_BitSize", V::UInt8(0)),
                    ],
                ),
            );
            s::set(&mut objects[6].value, "m_CompressedMesh", compressed);
            objects[2] = Object::new(
                3,
                1,
                s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 10)]),
            );
            objects.push(Object::new(10, 23, s::mesh_renderer((0, 3), &[], (0, 0))));
            add_healthy(&mut objects);
            let (env, graph) = h::graph(&objects);
            let meter = h::meter();
            let mut profile = Profile::new(&meter);
            profile.normalise = normalise;
            let scene = collect(&env, &graph, &profile, &meter).unwrap();
            assert_eq!(scene.collection_issues.len(), 2, "{attribute}");
            assert_eq!(
                scene.collection_issues.as_slice()[0].reason,
                "attribute count",
                "{attribute}"
            );
            assert!(scene.nodes.as_slice()[1].mesh.is_none());
            assert!(scene.nodes.as_slice()[0].mesh.is_some());
            assert_eq!(scene.accessors.len(), 2, "failed geometry rolled back");
            assert_eq!(extra(&scene, 1, "attribute count"), b"2");
            let (json, report) = document(&scene, &profile);
            assert_eq!(json["meshes"].as_array().unwrap().len(), 1);
            assert_eq!(report.collection_issues.len(), 2);
            h::validator_clean(&scene);
        }
    }
}

fn builtin_graph(objects: &[Object], options: EnvOptions) -> (Environment, Graph) {
    let env = Environment::new(options);
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(
                s::file(
                    s::Layout::Unity2022,
                    &["Library/unity default resources"],
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
fn finite_compressed_uvs_overflowing_f32_degrade_before_writing() {
    let mut objects = h::objects(false, false);
    let mut compressed = m::compressed();
    for (name, count, range, start, bits, data) in [
        ("m_Vertices", 9, 0., 0., 0, vec![]),
        ("m_UV", 6, f32::MAX, f32::MAX, 1, vec![V::UInt8(255)]),
    ] {
        s::set(
            &mut compressed,
            name,
            st(
                "PackedBitVector",
                vec![
                    ("m_NumItems", V::UInt(count)),
                    ("m_Range", V::Float(range)),
                    ("m_Start", V::Float(start)),
                    ("m_BitSize", V::UInt8(bits)),
                    ("m_Data", vector(V::UInt8(0), data)),
                ],
            ),
        );
    }
    s::set(&mut compressed, "m_UVInfo", V::UInt(5));
    s::set(&mut objects[6].value, "m_CompressedMesh", compressed);
    add_healthy(&mut objects);
    let (env, graph) = h::graph(&objects);
    let meter = h::meter();
    let profile = Profile::new(&meter);
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    assert!(scene.nodes.as_slice()[0].mesh.is_some());
    assert!(scene.nodes.as_slice()[1].mesh.is_none());
    assert_eq!(scene.collection_issues.len(), 1);
    assert_eq!(
        scene.collection_issues.as_slice()[0].reason,
        "invalid vertex attribute"
    );
    document(&scene, &profile);
    h::validator_clean(&scene);
}

#[test]
fn builtin_data_failures_leave_the_healthy_renderer_exportable() {
    for case in [
        "skin",
        "unsupported",
        "missing_file",
        "missing_object",
        "raw_file",
        "bad_mesh",
    ] {
        let mut objects = h::objects(case == "skin", false);
        if case == "skin" {
            objects[5] = Object::new(
                6,
                137,
                s::skinned_mesh_renderer((0, 3), (1, 10210), &[(0, 2)], &[]),
            );
        } else {
            objects[4] = Object::new(
                5,
                33,
                s::mesh_filter(
                    (0, 3),
                    (1, if case == "unsupported" { 10207 } else { 10210 }),
                ),
            );
        }
        add_healthy(&mut objects);
        let (env, graph) = builtin_graph(&objects, EnvOptions::unitypy());
        if matches!(case, "missing_object" | "raw_file" | "bad_mesh") {
            env.load(Source::Bytes {
                name: RawStr::from_static(b"unity default resources"),
                data: Bytes::from(if case == "raw_file" {
                    vec![0; 32]
                } else {
                    let mut mesh = plain_mesh(10210);
                    if case == "bad_mesh" {
                        s::set(&mut mesh.value, "m_IndexBuffer", V::Bytes(vec![0]));
                    }
                    s::file(
                        s::Layout::Unity2022,
                        &[],
                        if case == "missing_object" {
                            std::slice::from_ref(&objects[6])
                        } else {
                            std::slice::from_ref(&mesh)
                        },
                    )
                    .unwrap()
                }),
            })
            .unwrap();
        }
        let meter = h::meter();
        let mut profile = Profile::new(&meter);
        profile.builtin = if matches!(case, "skin" | "unsupported") {
            Builtin::Procedural
        } else {
            Builtin::Resolve
        };
        let scene = collect(&env, &graph, &profile, &meter).unwrap();
        assert!(scene.nodes.as_slice()[0].mesh.is_some(), "{case}");
        assert!(scene.nodes.as_slice()[1].mesh.is_none(), "{case}");
        let expected = match case {
            "skin" => "procedural builtin skin",
            "unsupported" => "procedural builtin supports Cube, Quad and Plane",
            "missing_file" | "raw_file" => "builtin resource unresolved",
            "missing_object" => "builtin object unresolved",
            _ => "mesh_decode_failed",
        };
        assert_eq!(
            extra(&scene, 1, "unresolved_renderer"),
            expected.as_bytes(),
            "{case}"
        );
        assert_eq!(scene.collection_issues.len(), 1);
        document(&scene, &profile);
        h::validator_clean(&scene);
    }
}

#[test]
fn repeated_builtin_renderers_keep_separate_attachments_and_aggregate_markers() {
    for mode in [Builtin::Placeholder, Builtin::Procedural, Builtin::Resolve] {
        let mut objects = h::objects(true, false);
        objects[2] = Object::new(
            3,
            1,
            s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 10), (0, 11)]),
        );
        objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
        objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[], (0, 0)));
        objects.extend([
            Object::new(10, 23, s::mesh_renderer((0, 3), &[], (0, 0))),
            Object::new(
                11,
                137,
                s::skinned_mesh_renderer((0, 3), (1, 10207), &[], &[]),
            ),
        ]);
        let (env, graph) = builtin_graph(&objects, EnvOptions::unitypy());
        let meter = h::meter();
        let mut profile = Profile::new(&meter);
        profile.builtin = mode;
        let scene = collect(&env, &graph, &profile, &meter).unwrap();
        assert_eq!(extra(&scene, 1, "unity_builtin"), b"10207;10210");
        let attachments = scene
            .nodes
            .as_slice()
            .iter()
            .filter(|n| n.mesh.is_some())
            .count();
        assert_eq!(attachments, if mode == Builtin::Procedural { 2 } else { 0 });
        if mode == Builtin::Procedural {
            assert_eq!(scene.nodes.as_slice()[1].translation, [-2., 0., 0.]);
            let child = scene.nodes.as_slice()[1].children.as_slice()[0];
            assert_eq!(scene.nodes.as_slice()[child].translation, [0.; 3]);
        }
        assert_eq!(
            scene.collection_issues.len(),
            match mode {
                Builtin::Procedural => 1,
                Builtin::Placeholder | Builtin::Resolve => 3,
            }
        );
        document(&scene, &profile);
        h::validator_clean(&scene);
    }
}

#[derive(Debug)]
struct Refuse(Error);
impl CabResolver for Refuse {
    fn resolve(&self, _: &[u8]) -> Option<Source> {
        None
    }
    fn find_file(&self, _: &[u8]) -> Result<Option<Source>, Error> {
        Err(self.0.clone())
    }
}
#[test]
fn builtin_resolution_preserves_nested_resource_refusals() {
    use unityrs_core::budget::{AllocError, BudgetExceeded, Reservation};
    for error in [
        Error::Budget(BudgetExceeded {
            requested: 1,
            what: Reservation::Input,
        }),
        Error::Allocator(AllocError {
            requested: 1,
            what: Reservation::Input,
        }),
        Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        Error::Limit(LimitError::refused(LimitKind::DecodeWork, 2, 1)),
        Error::Tree(unityrs_core::typetree::TreeError::Limit(
            LimitError::refused(LimitKind::TreeDepth, 2, 1),
        )),
        Error::Detect(unityrs_core::detect::DetectError::Work(
            LimitError::refused(LimitKind::DecodeWork, 2, 1),
        )),
    ] {
        let mut objects = h::objects(false, false);
        objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
        let mut options = EnvOptions::unitypy();
        options.resolver = Some(Arc::new(Refuse(error.clone())));
        let (env, graph) = builtin_graph(&objects, options);
        let meter = h::meter();
        let mut profile = Profile::new(&meter);
        profile.builtin = Builtin::Resolve;
        let Err(GltfError::Mesh(unityrs_mesh::MeshError::Core(actual))) =
            collect(&env, &graph, &profile, &meter)
        else {
            panic!("resource refusal swallowed")
        };
        assert_eq!(actual, error);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn builtin_resolution_recovers_nested_malformed_resource_data() {
    use unityrs_core::{
        env::DerefError,
        typetree::TreeError,
        walk::{ParseError, ReadDetail, ReadError, ReadErrorKind},
    };
    for error in [
        Error::from(std::io::Error::from(std::io::ErrorKind::NotFound)),
        Error::Detect(unityrs_core::detect::DetectError::Unterminated),
        Error::Tree(TreeError::Empty),
        Error::Deref(Arc::new(DerefError::MissingObject {
            file: RawStr::from_static(b"synthetic"),
            path_id: 7,
        })),
        Error::Entry {
            name: RawStr::from_static(b"synthetic"),
            source: Arc::new(Error::AssetBundle(Arc::new(ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Tree(TreeError::LevelUnderflow { row: 0, level: -1 }),
            })))),
        },
    ] {
        let mut objects = h::objects(false, false);
        objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
        add_healthy(&mut objects);
        let mut options = EnvOptions::unitypy();
        options.resolver = Some(Arc::new(Refuse(error)));
        let (env, graph) = builtin_graph(&objects, options);
        let meter = h::meter();
        let mut profile = Profile::new(&meter);
        profile.builtin = Builtin::Resolve;
        let scene = collect(&env, &graph, &profile, &meter).unwrap();
        assert!(scene.nodes.as_slice()[0].mesh.is_some());
        assert!(scene.nodes.as_slice()[1].mesh.is_none());
        assert_eq!(scene.collection_issues.len(), 1);
        assert_eq!(
            scene.collection_issues.as_slice()[0].reason,
            "mesh_resource_failed"
        );
        document(&scene, &profile);
        h::validator_clean(&scene);
    }
}
