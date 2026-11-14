use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_scene::{SceneError, accessors};
use unityrs_testkit::ngui::{Object, st};
use unityrs_testkit::scene3d::{self as s, Layout};

fn loaded(objects: &[Object]) -> (Environment, Arc<SerializedFile>) {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    (env, file)
}
#[test]
fn get_mesh_skips_null_components_and_filters_without_a_mesh() {
    let (env, f) = loaded(&[
        Object::new(1, 1, s::game_object(b"go", &[(0, 0), (0, 2), (0, 3)])),
        Object::new(2, 33, s::mesh_filter((0, 1), (0, 0))),
        Object::new(3, 33, s::mesh_filter((0, 1), (0, 9))),
        Object::new(4, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
        Object::new(5, 137, s::skinned_mesh_renderer((0, 1), (0, 0), &[], &[])),
        Object::new(9, 43, st("Mesh", vec![])),
    ]);
    let m = Meter::unlimited();
    assert_eq!(
        accessors::get_mesh(&f.object(4).unwrap(), &env, &m)
            .unwrap()
            .unwrap()
            .path_id(),
        9
    );
    assert!(
        accessors::get_mesh(&f.object(5).unwrap(), &env, &m)
            .unwrap()
            .is_none()
    );
}
#[test]
fn tier_b_empty_and_legacy_lists_and_exact_transform_class() {
    for legacy in [false, true] {
        let value = if legacy {
            s::legacy_game_object(b"go", &[(4, (0, 2)), (33, (0, 3)), (33, (0, 4))])
        } else {
            s::game_object(b"go", &[(0, 2), (0, 3), (0, 4)])
        };
        let (env, f) = loaded(&[
            Object::new(1, 1, value),
            Object::new(2, 224, st("RectTransform", vec![])),
            Object::new(3, 33, s::mesh_filter((0, 1), (0, 0))),
            Object::new(4, 33, s::mesh_filter((0, 1), (0, 0))),
            Object::new(5, 1, s::game_object(b"empty", &[])),
        ]);
        let m = Meter::unlimited();
        assert!(
            accessors::m_transform(&f.object(1).unwrap(), &env, &m)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            accessors::m_mesh_filter(&f.object(1).unwrap(), &env, &m)
                .unwrap()
                .unwrap()
                .path_id(),
            3
        );
        assert!(
            accessors::components(&f.object(5).unwrap().parse_as_dict(false).unwrap())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn object_mode_shape_refusals_are_tier_u_view_errors() {
    use unityrs_core::objmode::ViewError;
    let (env, f) = loaded(&[
        Object::new(1, 1, st("WrongGameObject", vec![])),
        Object::new(2, 23, st("WrongRenderer", vec![])),
        Object::new(3, 1, s::game_object(b"go", &[(0, 4)])),
        Object::new(4, 33, st("WrongFilter", vec![])),
        Object::new(5, 23, s::mesh_renderer((0, 3), &[], (0, 0))),
    ]);
    let meter = Meter::unlimited();
    let errors = [
        accessors::components(&f.object(1).unwrap().parse_as_dict(false).unwrap()).unwrap_err(),
        accessors::get_mesh(&f.object(2).unwrap(), &env, &meter).unwrap_err(),
        accessors::get_mesh(&f.object(5).unwrap(), &env, &meter).unwrap_err(),
    ];
    for error in errors {
        assert!(
            matches!(error, SceneError::View(ViewError::Malformed(_))),
            "{error:?}"
        );
    }
}
