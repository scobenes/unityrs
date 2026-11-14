use serde_json::Value as J;
use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, ObjectRef, RawStr};
use unityrs_scene::{GraphOptions, SceneError, accessors};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::ngui::{Object, V, st};
use unityrs_testkit::scene3d::{self as s, Layout};
use unityrs_testkit::serialized::{self, ObjectSpec, SerializedSpec, TypeSpec};
use unityrs_testkit::writer::ByteWriter;

fn load(objects: &[Object]) -> (Environment, Arc<SerializedFile>) {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(f) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"oracle"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    (env, f)
}
fn fixture() -> J {
    serde_json::from_str(include_str!("fixtures/accessors.json")).unwrap()
}
type Accessor = fn(&ObjectRef, &Environment, &Arc<Meter>) -> Result<Option<ObjectRef>, SceneError>;
fn accessors_list() -> [(&'static str, Accessor); 6] {
    [
        ("m_Transform", accessors::m_transform),
        ("m_Animator", accessors::m_animator),
        ("m_Animation", accessors::m_animation),
        ("m_MeshRenderer", accessors::m_mesh_renderer),
        ("m_SkinnedMeshRenderer", accessors::m_skinned_mesh_renderer),
        ("m_MeshFilter", accessors::m_mesh_filter),
    ]
}
#[test]
fn accessor_values_and_tier_b_dict_answers() {
    for c in fixture()["cases"].as_array().unwrap() {
        let input: Vec<_> = c["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    i32::try_from(v[0].as_i64().unwrap()).unwrap(),
                    v[1].as_i64().unwrap(),
                )
            })
            .collect();
        let form = c["form"].as_str().unwrap();
        let mut go = if form == "legacy" {
            s::legacy_game_object(
                b"go",
                &input
                    .iter()
                    .map(|(cls, id)| (*cls, (0, *id)))
                    .collect::<Vec<_>>(),
            )
        } else {
            s::game_object(
                b"go",
                &input.iter().map(|(_, id)| (0, *id)).collect::<Vec<_>>(),
            )
        };
        if form == "none" {
            let V::Struct(_, fields) = &mut go else {
                panic!("go")
            };
            fields.retain(|(n, _)| n != "m_Component");
        }
        let mut objects = vec![Object::new(1, 1, go)];
        objects.extend(
            input
                .iter()
                .map(|(cls, id)| Object::new(*id, *cls, st("Component", vec![]))),
        );
        let (env, f) = load(&objects);
        let go = f.object(1).unwrap();
        let p = go.parse_as_dict(false).unwrap();
        let actual = accessors::components(&p).unwrap();
        let ids: Vec<_> = actual.as_slice().iter().map(|p| p.path_id).collect();
        assert_eq!(
            serde_json::to_value(ids).unwrap(),
            c["answer"]["components"],
            "{}",
            c["case"]
        );
        for (name, call) in accessors_list() {
            let value = call(&go, &env, &Meter::unlimited())
                .unwrap()
                .map(|o| o.path_id());
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                c["answer"][name],
                "{} {name}",
                c["case"]
            );
            if matches!(form, "legacy" | "empty") {
                assert!(c["oracle"][name].get("error").is_some());
            } else {
                assert_eq!(c["oracle"][name]["value"], c["answer"][name]);
            }
        }
    }
}
#[test]
fn get_mesh_matches_expected_oracle_values() {
    for c in fixture()["mesh_cases"].as_array().unwrap() {
        let filters = c["filters"].as_array().unwrap();
        let mut ptrs = vec![(0, 0)];
        let mut objects = vec![Object::new(9, 43, st("Mesh", vec![]))];
        for (i, mesh) in filters.iter().enumerate() {
            let id = 20 + i64::try_from(i).unwrap();
            ptrs.push((0, id));
            objects.push(Object::new(
                id,
                33,
                s::mesh_filter((0, 1), (0, mesh.as_i64().unwrap())),
            ));
        }
        let go = if c["legacy"] == true {
            s::legacy_game_object(b"go", &ptrs.iter().map(|p| (33, *p)).collect::<Vec<_>>())
        } else {
            s::game_object(b"go", &ptrs)
        };
        objects.push(Object::new(1, 1, go));
        let (class, value) = if c["skin"] == true {
            (
                137,
                s::skinned_mesh_renderer((0, 1), (0, c["mesh"].as_i64().unwrap()), &[], &[]),
            )
        } else {
            (23, s::mesh_renderer((0, 1), &[], (0, 0)))
        };
        objects.push(Object::new(2, class, value));
        let (env, f) = load(&objects);
        let actual = accessors::get_mesh(&f.object(2).unwrap(), &env, &Meter::unlimited())
            .unwrap()
            .map(|o| o.path_id());
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            c["oracle"]["value"],
            "{}",
            c["case"]
        );
    }
}

fn collision_file(input: &J, class_id: i32, path_id: i64, objects: &[Object]) -> SerializedSpec {
    let mut spec = SerializedSpec::default();
    for object in objects {
        let mut writer = ByteWriter::le();
        unityrs_testkit::ngui::write(&mut writer, &object.value).unwrap();
        spec.objects.push(ObjectSpec {
            path_id: object.path_id,
            type_id: i32::try_from(spec.types.len()).unwrap(),
            data: writer.into_inner(),
            ..ObjectSpec::default()
        });
        spec.types.push(TypeSpec {
            class_id: object.class_id,
            nodes: unityrs_testkit::ngui::nodes(0, "Base", &object.value).unwrap(),
            ..TypeSpec::default()
        });
    }
    let nodes = input["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut node = NodeSpec::new(
                r[0].as_u64().unwrap().try_into().unwrap(),
                r[1].as_str().unwrap(),
                r[2].as_str().unwrap(),
            )
            .size(-1);
            node.meta_flag = r[3].as_i64().unwrap().try_into().unwrap();
            node
        })
        .collect();
    let hex = input["data"].as_str().unwrap();
    let data = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    spec.objects.push(ObjectSpec {
        path_id,
        type_id: spec.types.len().try_into().unwrap(),
        data,
        ..ObjectSpec::default()
    });
    spec.types.push(TypeSpec {
        class_id,
        nodes,
        ..TypeSpec::default()
    });
    spec
}

#[test]
fn recorded_mesh_collision_selects_the_last_child() {
    replay_collision(false);
}

#[test]
fn recorded_component_collision_selects_the_last_child() {
    replay_collision(true);
}

fn replay_collision(want_components: bool) {
    let cases = fixture();
    let cases = cases["collision_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let component_collision = case["oracle"].get("components").is_some();
        if component_collision != want_components {
            continue;
        }
        let mut objects = vec![
            Object::new(2, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
            Object::new(3, 4, s::transform((0, 1), (0, 0), &[], false)),
            Object::new(9, 43, st("Mesh", vec![])),
            Object::new(10, 43, st("Mesh", vec![])),
        ];
        let (class, path_id) = if component_collision {
            objects.extend([
                Object::new(20, 33, s::mesh_filter((0, 1), (0, 9))),
                Object::new(21, 33, s::mesh_filter((0, 1), (0, 10))),
            ]);
            (1, 1)
        } else {
            objects.push(Object::new(1, 1, s::game_object(b"go", &[(0, 20)])));
            (33, 20)
        };
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"collision"),
                data: Bytes::from(serialized::build(&collision_file(
                    &case["input"],
                    class,
                    path_id,
                    &objects,
                ))),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let meter = Meter::unlimited();
        if component_collision {
            let go = file.object(1).unwrap();
            let parsed = go.parse_as_dict(false).unwrap();
            let ptrs = accessors::components(&parsed).unwrap();
            let ids: Vec<_> = ptrs.as_slice().iter().map(|p| p.path_id).collect();
            assert_eq!(
                serde_json::to_value(ids).unwrap(),
                case["oracle"]["components"]["value"]
            );
            let filter = accessors::m_mesh_filter(&go, &env, &meter)
                .unwrap()
                .unwrap();
            assert_eq!(
                filter.path_id(),
                case["oracle"]["m_MeshFilter"]["value"].as_i64().unwrap()
            );
        }
        let mesh = accessors::get_mesh(&file.object(2).unwrap(), &env, &meter)
            .unwrap()
            .unwrap();
        assert_eq!(
            mesh.path_id(),
            case["oracle"]["get_mesh"]["value"].as_i64().unwrap(),
            "{}",
            case["case"]
        );
        let graph =
            unityrs_scene::graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter)
                .unwrap();
        let node = &graph.nodes()[0];
        let unityrs_scene::Component::MeshFilter { object, mesh } = &node.components.as_slice()[0]
        else {
            panic!("filter")
        };
        assert_eq!(object.path_id(), 20, "graph facts keep raw component names");
        let Some(unityrs_scene::PPtrTarget::Object(mesh)) = mesh else {
            panic!("mesh")
        };
        assert_eq!(mesh.path_id(), 9, "graph facts keep raw mesh names");
    }
}
