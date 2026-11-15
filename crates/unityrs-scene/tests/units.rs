use std::sync::Arc;
use unityrs_core::{
    Bytes, RawStr,
    container::ContainerRow,
    env::{EnvOptions, Environment, Source},
    files::{File, SerializedFile},
    limits::Meter,
};
use unityrs_scene::{GraphOptions, SceneError, Unit, unit_from_container, unit_from_object};
use unityrs_testkit::{
    ngui::{Object, st},
    scene3d as s,
};

fn load() -> (Environment, Arc<SerializedFile>) {
    // Transform order deliberately differs from signed path-id order.
    let objects = [
        Object::new(90, 4, s::transform((0, 9), (0, 0), &[(0, 20)], false)),
        Object::new(9, 1, s::game_object(b"first", &[(0, 90)])),
        Object::new(-5, 4, s::transform((0, 8), (0, 0), &[], false)),
        Object::new(8, 1, s::game_object(b"second", &[(0, -5)])),
        Object::new(20, 4, s::transform((0, 2), (0, 90), &[], false)),
        Object::new(2, 1, s::game_object(b"child", &[(0, 20)])),
        // A marker needs no readable tree: selecting it selects the file.
        Object::new(50, 1032, st("SceneAsset", vec![])),
    ];
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"BuildPlayer-synthetic"),
            data: Bytes::from(s::file(s::Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    (env, file)
}

#[test]
fn scene_marker_selects_whole_file_while_prefab_selects_only_its_root() {
    let (env, file) = load();
    let meter = Meter::unlimited();
    let scene = unit_from_object(&file.object(50).unwrap()).unwrap();
    assert!(matches!(scene, Unit::Scene(_)));
    assert!(Arc::ptr_eq(scene.meter(), file.meter()));
    let graph = scene.graph(&env, &GraphOptions::default(), &meter).unwrap();
    assert_eq!(graph.nodes().len(), 3);
    let roots: Vec<_> = graph
        .roots()
        .iter()
        .map(|&i| graph.nodes()[i].transform.path_id())
        .collect();
    assert_eq!(roots, [90, -5]);
    let prefab = unit_from_object(&file.object(9).unwrap()).unwrap();
    assert!(matches!(prefab, Unit::Prefab(_)));
    let graph = prefab
        .graph(&env, &GraphOptions::default(), &meter)
        .unwrap();
    assert_eq!(graph.nodes().len(), 2);
    assert_eq!(graph.roots(), [0]);
    assert!(matches!(
        unit_from_object(&file.object(90).unwrap()),
        Err(SceneError::WrongClass(4))
    ));
}

fn row(file_id: i128, path_id: i128) -> ContainerRow {
    ContainerRow {
        path: Some(RawStr::from_static(b"assets/example.prefab")),
        preload_index: Some(0),
        preload_size: Some(0),
        file_id: Some(file_id),
        path_id: Some(path_id),
    }
}

#[test]
fn container_entries_target_gameobjects_and_keep_root_selection() {
    let (env, file) = load();
    let prefab = unit_from_container(&env, &file, &row(0, 2))
        .unwrap()
        .unwrap();
    let graph = prefab
        .graph(&env, &GraphOptions::default(), &Meter::unlimited())
        .unwrap();
    assert_eq!(graph.nodes().len(), 1);
    assert_eq!(graph.nodes()[0].game_object.path_id(), 2);
    assert!(
        unit_from_container(&env, &file, &row(0, 90))
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        unit_from_container(&env, &file, &row(0, 50)).unwrap(),
        Some(Unit::Scene(_))
    ));
    assert!(unit_from_container(&env, &file, &row(0, 999)).is_err());
    assert!(unit_from_container(&env, &file, &row(i128::MAX, 2)).is_err());
}

#[test]
fn container_entry_resolves_an_external_prefab_root() {
    let (env, target) = load();
    let File::Serialized(source) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"CAB-index"),
            data: Bytes::from(
                s::file(s::Layout::Unity2022, &["BuildPlayer-synthetic"], &[]).unwrap(),
            ),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let Unit::Prefab(root) = unit_from_container(&env, &source, &row(1, 9))
        .unwrap()
        .unwrap()
    else {
        panic!("prefab")
    };
    assert!(Arc::ptr_eq(root.file(), &target));
}
