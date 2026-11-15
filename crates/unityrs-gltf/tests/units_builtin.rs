mod collect_support;
use collect_support as h;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use unityrs_core::{
    Bytes, RawStr,
    env::{CabResolver, EnvOptions, Environment, Source},
    files::{File, SerializedFile},
    limits::Meter,
};
use unityrs_gltf::*;
use unityrs_scene::{GraphOptions, Unit, unit_from_object};
use unityrs_testkit::{
    ngui::{Object, st},
    scene3d as s,
};

fn load(
    env: &Environment,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
) -> Arc<SerializedFile> {
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(name),
            data: Bytes::from(s::file(s::Layout::Unity2022, externals, objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    file
}

#[derive(Debug)]
struct Resolver {
    bytes: Bytes,
    calls: Arc<AtomicUsize>,
}
impl CabResolver for Resolver {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        (name.rsplit(|b| matches!(b, b'/' | b'\\')).next()
            == Some(b"unity default resources".as_slice()))
        .then(|| Source::Bytes {
            name: RawStr::from_static(b"supplied-file"),
            data: self.bytes.clone(),
        })
    }
}

fn resolved_mesh() -> Object {
    s::mesh::mesh(
        10210,
        b"supplied",
        &[[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    )
}

#[test]
fn builtin_resolve_uses_the_normal_external_basename_with_cab_resolver() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut opts = EnvOptions::unitypy();
    opts.resolver = Some(Arc::new(Resolver {
        bytes: Bytes::from(s::file(s::Layout::Unity2022, &[], &[resolved_mesh()]).unwrap()),
        calls: calls.clone(),
    }));
    let env = Environment::new(opts);
    let mut objects = h::objects(false, false);
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    let file = load(
        &env,
        b"level0",
        &["Library/unity default resources"],
        &objects,
    );
    let meter = Meter::unlimited();
    let graph = Unit::Scene(file)
        .graph(&env, &GraphOptions::default(), &meter)
        .unwrap();
    let mut profile = Profile::new(&meter);
    // The default placeholder must never consult a configured resolver.
    let placeholder = collect(&env, &graph, &profile, &meter).unwrap();
    assert!(placeholder.meshes.is_empty());
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(
        placeholder
            .collection_issues
            .as_slice()
            .iter()
            .any(|i| i.reason == "builtin_placeholder")
    );
    h::validator_clean(&placeholder);
    profile.builtin = Builtin::Resolve;
    let resolved = collect(&env, &graph, &profile, &meter).unwrap();
    assert_eq!(resolved.meshes.len(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        h::floats(&resolved, 0),
        &[0., 0., 0., -2., 0., 0., 0., 2., 0.]
    );
    h::validator_clean(&resolved);
    // Loading a resource cannot change the explicitly selected default mode.
    profile.builtin = Builtin::Placeholder;
    assert!(
        collect(&env, &graph, &profile, &meter)
            .unwrap()
            .meshes
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn builtin_backslash_aliases_resolve_once_and_share_one_mesh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut opts = EnvOptions::unitypy();
    opts.resolver = Some(Arc::new(Resolver {
        bytes: Bytes::from(s::file(s::Layout::Unity2022, &[], &[resolved_mesh()]).unwrap()),
        calls: calls.clone(),
    }));
    let env = Environment::new(opts);
    let mut objects = h::objects(false, false);
    objects[0] = Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 10), (0, 11)]));
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    objects.extend([
        Object::new(10, 33, s::mesh_filter((0, 1), (2, 10210))),
        Object::new(11, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
    ]);
    let file = load(
        &env,
        b"level0",
        &[r"A\unity default resources", r"B\UNITY DEFAULT RESOURCES"],
        &objects,
    );
    let meter = Meter::unlimited();
    let graph = Unit::Scene(file)
        .graph(&env, &GraphOptions::default(), &meter)
        .unwrap();
    let mut profile = Profile::new(&meter);
    profile.builtin = Builtin::Resolve;
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(scene.nodes.as_slice()[0].mesh, Some(0));
    assert_eq!(scene.nodes.as_slice()[1].mesh, Some(0));
    h::validator_clean(&scene);
}

#[test]
fn structured_builtin_identity_preserves_resource_names_and_unknown_ids() {
    for (external, id, file, name) in [
        (1, 10202, "unity default resources", Some("Cube")),
        (1, 10206, "unity default resources", Some("Cylinder")),
        (1, 10207, "unity default resources", Some("Sphere")),
        (1, 10208, "unity default resources", Some("Capsule")),
        (1, 10209, "unity default resources", Some("Plane")),
        (1, 10210, "unity default resources", Some("Quad")),
        (2, 10303, "unity_builtin_extra", Some("Default-Material")),
        (1, -777, "unity default resources", None),
        (2, -777, "unity_builtin_extra", None),
    ] {
        let env = Environment::new(EnvOptions::unitypy());
        let mut objects = h::objects(false, false);
        objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (external, id)));
        let file_ref = load(
            &env,
            b"level0",
            &[
                r"A\UNITY DEFAULT RESOURCES",
                "Resources/UNITY_BUILTIN_EXTRA",
            ],
            &objects,
        );
        let meter = Meter::unlimited();
        let graph = Unit::Scene(file_ref)
            .graph(&env, &GraphOptions::default(), &meter)
            .unwrap();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        assert!(scene.meshes.is_empty());
        let bytes = h::write(&scene);
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
        assert_eq!(
            json["nodes"][1]["extras"]["unity_builtin"],
            serde_json::json!([
                {"file": file, "id": id, "name": name}
            ])
        );
    }
}

#[test]
fn structured_builtin_metadata_cannot_duplicate_a_string_extra() {
    let meter = Meter::unlimited();
    let mut scene = Scene::new(&meter);
    let mut node = Node::new(&meter);
    node.builtins
        .push(BuiltinIdentity {
            file: "unity default resources",
            id: 10210,
            name: Some("Quad"),
        })
        .unwrap();
    node.extras
        .push((
            text(&meter, "unity_builtin").unwrap(),
            text(&meter, "conflict").unwrap(),
        ))
        .unwrap();
    scene.nodes.push(node).unwrap();
    scene.roots.push(0).unwrap();
    let mut bytes = Vec::new();
    assert!(write_glb(&scene, &Options::default(), &meter, &mut bytes).is_err());
    assert_eq!(bytes, [] as [u8; 0]);
}

#[test]
fn builtin_identity_order_and_uniqueness_are_checked_before_writing() {
    let meter = Meter::unlimited();
    let quad = BuiltinIdentity {
        file: "unity default resources",
        id: 10210,
        name: Some("Quad"),
    };
    let sphere = BuiltinIdentity {
        file: "unity default resources",
        id: 10207,
        name: Some("Sphere"),
    };
    for identities in [[quad, quad], [quad, sphere]] {
        let mut scene = Scene::new(&meter);
        let mut node = Node::new(&meter);
        node.builtins.extend_from_slice(&identities).unwrap();
        scene.nodes.push(node).unwrap();
        scene.roots.push(0).unwrap();
        let mut bytes = Vec::new();
        assert!(write_glb(&scene, &Options::default(), &meter, &mut bytes).is_err());
        assert_eq!(bytes, [] as [u8; 0]);
    }
}

#[test]
fn every_procedural_primitive_is_approximate_bounded_and_validator_clean() {
    for (path_id, vertices, triangles, extent) in
        [(10202, 8, 12, 0.5), (10209, 4, 2, 5.), (10210, 4, 2, 0.5)]
    {
        let env = Environment::new(EnvOptions::unitypy());
        let mut objects = h::objects(false, false);
        objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, path_id)));
        let file = load(
            &env,
            b"level0",
            &["Library/unity default resources"],
            &objects,
        );
        let meter = Meter::unlimited();
        let graph = Unit::Scene(file)
            .graph(&env, &GraphOptions::default(), &meter)
            .unwrap();
        let mut profile = Profile::new(&meter);
        profile.builtin = Builtin::Procedural;
        let scene = collect(&env, &graph, &profile, &meter).unwrap();
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.accessors.as_slice()[0].count, vertices);
        assert_eq!(scene.accessors.as_slice()[1].count, triangles * 3);
        assert!(h::floats(&scene, 0).iter().all(|x| x.abs() <= extent));
        for extras in [
            &scene.meshes.as_slice()[0].extras,
            &scene.nodes.as_slice()[1].extras,
        ] {
            assert!(
                extras
                    .as_slice()
                    .iter()
                    .any(|(k, v)| k.as_bytes() == b"approximate" && v.as_bytes() == b"true")
            );
        }
        h::validator_clean(&scene);
        assert_eq!(h::write(&scene), h::write(&scene));
    }
}

#[test]
fn scene_and_prefab_units_keep_root_order_and_apply_filters_during_collect() {
    let env = Environment::new(EnvOptions::unitypy());
    let objects = [
        Object::new(90, 4, s::transform((0, 9), (0, 0), &[(0, 20)], false)),
        Object::new(9, 1, s::game_object(b"first", &[(0, 90)])),
        Object::new(-5, 4, s::transform((0, 8), (0, 0), &[], false)),
        Object::new(8, 1, s::game_object(b"second", &[(0, -5)])),
        Object::new(20, 4, s::transform((0, 2), (0, 90), &[], false)),
        Object::new(2, 1, s::game_object(b"child", &[(0, 20)])),
        Object::new(50, 1032, st("SceneAsset", vec![])),
    ];
    let file = load(&env, b"BuildPlayer-demo", &[], &objects);
    // A shared-assets dependency with its own roots never joins this unit.
    load(
        &env,
        b"BuildPlayer-demo.sharedAssets",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"dependency", &[(0, 2)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
        ],
    );
    let meter = Meter::unlimited();
    let graph = unit_from_object(&file.object(50).unwrap())
        .unwrap()
        .graph(&env, &GraphOptions::default(), &meter)
        .unwrap();
    let mut profile = Profile::new(&meter);
    let scene = collect(&env, &graph, &profile, &meter).unwrap();
    let names: Vec<_> = scene
        .roots
        .as_slice()
        .iter()
        .map(|&id| scene.nodes.as_slice()[id].name.as_ref().unwrap().as_bytes())
        .collect();
    assert_eq!(names, [b"first".as_slice(), b"second"]);
    h::validator_clean(&scene);
    profile
        .scene_roots
        .push(text(&meter, "first").unwrap())
        .unwrap();
    profile
        .exclusions
        .push(text(&meter, "first/child").unwrap())
        .unwrap();
    let filtered = collect(&env, &graph, &profile, &meter).unwrap();
    assert_eq!(filtered.roots.len(), 1);
    assert_eq!(filtered.nodes.len(), 1);
    h::validator_clean(&filtered);
    let prefab = unit_from_object(&file.object(2).unwrap())
        .unwrap()
        .graph(&env, &GraphOptions::default(), &meter)
        .unwrap();
    let scene = collect(&env, &prefab, &Profile::new(&meter), &meter).unwrap();
    assert_eq!(scene.nodes.len(), 1);
    assert_eq!(
        scene.nodes.as_slice()[0].name.as_ref().unwrap().as_bytes(),
        b"child"
    );
    h::validator_clean(&scene);
}
