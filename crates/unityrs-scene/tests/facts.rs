use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, PPtr, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_scene::{
    Component, GraphOptions, Issue, PPtrTarget, TransformKind,
    builtin::{self, BuiltinKind},
    graph_from_game_object,
};
use unityrs_testkit::ngui::{Object, V, st};
use unityrs_testkit::scene3d::{self as s, Layout};

fn load(
    env: &Environment,
    layout: Layout,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
) -> Arc<SerializedFile> {
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(name),
            data: Bytes::from(s::file(layout, externals, objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    file
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture across three engine versions"
)]
fn renderer_skin_lod_and_cross_file_facts_on_three_versions() {
    for layout in [Layout::Unity2019, Layout::Unity2022, Layout::Unity6000] {
        let env = Environment::new(EnvOptions::unitypy());
        let other = load(
            &env,
            layout,
            b"other",
            &[],
            &[
                Object::new(9, 43, st("Mesh", vec![])),
                Object::new(8, 21, st("Material", vec![])),
            ],
        );
        let mut renderer = s::mesh_renderer((0, 1), &[(1, 8), (0, 0), (0, 777)], (2, 3));
        s::set(
            &mut renderer,
            "m_SubsetIndices",
            unityrs_testkit::ngui::vector(V::UInt(0), vec![V::UInt(4), V::UInt(7)]),
        );
        s::set(&mut renderer, "m_Enabled", V::Bool(false));
        s::set(
            &mut renderer,
            "m_StaticBatchRoot",
            unityrs_testkit::ngui::pptr("Transform", (0, 10)),
        );
        let mut go = s::game_object(b"raw\xff", &[(0, 10), (0, 2), (0, 3), (0, 4), (0, 5)]);
        s::set(&mut go, "m_IsActive", V::UInt8(0));
        // A cleaned-name collision must not shadow the raw graph fact.
        s::set(&mut go, "m.Name", V::Str(b"shadow".to_vec()));
        let file = load(
            &env,
            layout,
            b"scene",
            &["archive:/other/other"],
            &[
                Object::new(1, 1, go),
                Object::new(10, 224, s::transform((0, 1), (0, 0), &[], true)),
                Object::new(2, 33, s::mesh_filter((0, 1), (1, 9))),
                Object::new(3, 23, renderer),
                Object::new(
                    4,
                    137,
                    s::skinned_mesh_renderer((0, 1), (1, 9), &[(0, 10), (0, 0)], &[12.5, -3.25]),
                ),
                Object::new(
                    5,
                    205,
                    s::lod_group((0, 1), &[(0.6, vec![(0, 3)]), (0.2, vec![(0, 4)])]),
                ),
            ],
        );
        let graph = graph_from_game_object(
            &env,
            &file.object(1).unwrap(),
            &GraphOptions::default(),
            &Meter::unlimited(),
        )
        .unwrap();
        let node = &graph.nodes()[0];
        assert_eq!(node.name.as_bytes(), b"raw\xff");
        assert!(!node.active);
        assert_eq!(node.kind, TransformKind::RectTransform);
        let Component::MeshFilter {
            mesh: Some(PPtrTarget::Object(mesh)),
            ..
        } = &node.components.as_slice()[1]
        else {
            panic!("filter")
        };
        assert!(Arc::ptr_eq(mesh.file(), &other));
        let Component::MeshRenderer(r) = &node.components.as_slice()[2] else {
            panic!("renderer")
        };
        assert_eq!(r.static_batch, Some((2, 3)));
        assert_eq!(r.subset_indices.as_slice(), [4, 7]);
        assert!(!r.enabled);
        assert!(matches!(&r.static_batch_root, Some(PPtrTarget::Object(t)) if t.path_id() == 10));
        assert!(
            matches!(&r.materials.as_slice()[0], Some(PPtrTarget::Object(t)) if t.path_id() == 8)
        );
        assert!(r.materials.as_slice()[1].is_none());
        assert!(matches!(
            &r.materials.as_slice()[2],
            Some(PPtrTarget::Unresolved(_))
        ));
        let Component::SkinnedMeshRenderer(skin) = &node.components.as_slice()[3] else {
            panic!("skin")
        };
        assert_eq!(skin.blend_weights.as_slice(), [12.5, -3.25]);
        assert_eq!(skin.bones.len(), 2);
        assert!(skin.bones.as_slice()[1].is_none());
        let Component::LodGroup(lod) = &node.components.as_slice()[4] else {
            panic!("LOD")
        };
        assert_eq!(lod.size, 2.);
        assert_eq!(lod.lods.as_slice()[0].screen_relative_height, 0.6);
        assert_eq!(lod.lods.as_slice()[1].fade_transition_width, 0.1);
        assert!(
            graph
                .issues()
                .iter()
                .any(|i| matches!(i, Issue::Unresolved { pointer, .. } if pointer.path_id == 777))
        );
    }
}

#[test]
fn builtin_recognition_preserves_unknown_ids_and_negative_file_indexing() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        Layout::Unity2022,
        b"builtins",
        &[
            "Library/UNITY DEFAULT RESOURCES",
            "Resources\\unity_builtin_extra",
        ],
        &[
            Object::new(1, 1, s::game_object(b"builtin", &[(0, 10), (0, 2)])),
            Object::new(10, 4, s::transform((0, 1), (0, 0), &[], false)),
            Object::new(2, 33, s::mesh_filter((0, 1), (1, 10202))),
        ],
    );
    let recognize = |pointer| builtin::recognize(&file, pointer, &Meter::unlimited()).unwrap();
    for (id, kind) in [
        (10202, BuiltinKind::Cube),
        (10206, BuiltinKind::Cylinder),
        (10207, BuiltinKind::Sphere),
        (10208, BuiltinKind::Capsule),
        (10209, BuiltinKind::Plane),
        (10210, BuiltinKind::Quad),
    ] {
        assert_eq!(
            recognize(PPtr {
                file_id: 1,
                path_id: id
            })
            .unwrap()
            .kind,
            Some(kind)
        );
    }
    assert_eq!(
        recognize(PPtr {
            file_id: -1,
            path_id: 10202
        })
        .unwrap()
        .kind,
        Some(BuiltinKind::Cube)
    );
    assert_eq!(
        recognize(PPtr {
            file_id: 2,
            path_id: 10303
        })
        .unwrap()
        .kind,
        Some(BuiltinKind::DefaultMaterial)
    );
    assert!(
        recognize(PPtr {
            file_id: 1,
            path_id: 123
        })
        .unwrap()
        .kind
        .is_none()
    );
    assert!(
        recognize(PPtr {
            file_id: i32::MIN,
            path_id: 123
        })
        .is_none()
    );
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    assert!(matches!(
        &graph.nodes()[0].components.as_slice()[1],
        Component::MeshFilter {
            mesh: Some(PPtrTarget::Builtin(_)),
            ..
        }
    ));
    assert!(matches!(graph.issues(), [Issue::Builtin { .. }]));
}

#[test]
fn failed_father_keeps_the_dereference_reason_beside_the_mismatch() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        Layout::Unity2022,
        b"father",
        &["missing"],
        &[
            Object::new(1, 1, s::game_object(b"root", &[(0, 10)])),
            Object::new(10, 4, s::transform((0, 1), (0, 0), &[(0, 20)], false)),
            Object::new(2, 1, s::game_object(b"child", &[(0, 20)])),
            Object::new(20, 4, s::transform((0, 2), (1, 123), &[], false)),
        ],
    );
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    assert!(graph.issues().iter().any(|i| matches!(i, Issue::Unresolved { pointer, reason: unityrs_core::env::DerefError::NotFound { .. }, .. } if pointer.file_id == 1 && pointer.path_id == 123)));
    assert!(
        graph
            .issues()
            .iter()
            .any(|i| matches!(i, Issue::ParentMismatch { .. }))
    );
}
