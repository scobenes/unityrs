use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_scene::{GraphOptions, Issue, graph_from_game_object, graph_from_scene_file};
use unityrs_testkit::ngui::{Object, V, file, pptr, st, vector};

fn go(id: i64, transform: i64) -> Object {
    Object::new(
        id,
        1,
        st(
            "GameObject",
            vec![
                ("m_Name", V::Str(format!("node-{id}").into_bytes())),
                ("m_IsActive", V::Bool(true)),
                ("m_Layer", V::UInt(7)),
                ("m_Tag", V::UInt(3)),
                (
                    "m_Component",
                    vector(
                        st(
                            "ComponentPair",
                            vec![("component", pptr("Component", (0, 0)))],
                        ),
                        vec![st(
                            "ComponentPair",
                            vec![("component", pptr("Component", (0, transform)))],
                        )],
                    ),
                ),
            ],
        ),
    )
}
fn xyz(ty: &str, values: &[f32]) -> V {
    let names = ["x", "y", "z", "w"];
    st(
        ty,
        names
            .iter()
            .zip(values)
            .map(|(n, v)| (*n, V::Float(*v)))
            .collect(),
    )
}
fn tr(id: i64, game_object: i64, father: i64, children: &[i64]) -> Object {
    Object::new(
        id,
        4,
        st(
            "Transform",
            vec![
                ("m_GameObject", pptr("GameObject", (0, game_object))),
                ("m_Father", pptr("Transform", (0, father))),
                (
                    "m_Children",
                    vector(
                        pptr("Transform", (0, 0)),
                        children
                            .iter()
                            .map(|p| pptr("Transform", (0, *p)))
                            .collect(),
                    ),
                ),
                ("m_LocalPosition", xyz("Vector3f", &[1., 2., 3.])),
                ("m_LocalRotation", xyz("Quaternionf", &[0., 0., 0., 1.])),
                ("m_LocalScale", xyz("Vector3f", &[1., 1., 1.])),
            ],
        ),
    )
}
fn load(objects: &[Object]) -> (Environment, Arc<SerializedFile>) {
    let env = Environment::new(EnvOptions::unitypy());
    let f = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(file(&[], objects, true).unwrap()),
        })
        .unwrap();
    let File::Serialized(f) = f else {
        panic!("serialized fixture")
    };
    (env, f)
}

#[test]
fn roots_follow_object_table_and_children_follow_their_list() {
    let (env, f) = load(&[
        tr(90, 9, 0, &[30, 20]),
        go(9, 90),
        tr(-5, 8, 0, &[]),
        go(8, -5),
        tr(20, 2, 90, &[]),
        go(2, 20),
        tr(30, 3, 90, &[]),
        go(3, 30),
    ]);
    let g = graph_from_scene_file(&env, &f, &GraphOptions::default(), &Meter::unlimited()).unwrap();
    assert_eq!(
        g.roots()
            .iter()
            .map(|i| g.nodes()[*i].transform.path_id())
            .collect::<Vec<_>>(),
        [90, -5]
    );
    let n = &g.nodes()[0];
    assert_eq!(
        n.children
            .as_slice()
            .iter()
            .map(|i| g.nodes()[*i].transform.path_id())
            .collect::<Vec<_>>(),
        [30, 20]
    );
    assert_eq!((n.layer, n.tag, n.active), (7, 3, true));
    assert_eq!(n.local.position, [1., 2., 3.]);
    assert!(g.issues().is_empty());
}

#[test]
fn cycles_missing_children_and_parent_mismatch_are_issues() {
    let (env, f) = load(&[
        go(1, 10),
        tr(10, 1, 0, &[20, 999]),
        go(2, 20),
        tr(20, 2, 0, &[10]),
    ]);
    let g = graph_from_game_object(
        &env,
        &f.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    assert_eq!(g.nodes().len(), 2);
    assert!(g.issues().iter().any(|i| matches!(i, Issue::Cycle { .. })));
    assert!(
        g.issues()
            .iter()
            .any(|i| matches!(i, Issue::ParentMismatch { .. }))
    );
    assert!(
        g.issues()
            .iter()
            .any(|i| matches!(i, Issue::Unresolved { .. }))
    );
}

#[test]
fn depth_is_bounded_and_the_root_must_resolve() {
    let (env, f) = load(&[
        go(1, 10),
        tr(10, 1, 0, &[20]),
        go(2, 20),
        tr(20, 2, 10, &[]),
        go(3, 999),
    ]);
    let opts = GraphOptions { max_depth: Some(1) };
    let g =
        graph_from_game_object(&env, &f.object(1).unwrap(), &opts, &Meter::unlimited()).unwrap();
    assert_eq!(g.nodes().len(), 1);
    assert!(g.issues().iter().any(|i| matches!(i, Issue::Depth { .. })));
    assert!(
        graph_from_game_object(&env, &f.object(3).unwrap(), &opts, &Meter::unlimited()).is_err()
    );
}
