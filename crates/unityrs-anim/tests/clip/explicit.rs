use crate::support::{bind, load, rest};
use unityrs_anim::{AnimError, ClipField, read_clip, sample_clip};
use unityrs_core::{limits::Meter, value::Value};
use unityrs_testkit::{
    ngui::{V, st},
    scene3d::{anim::playback as f, set},
};

#[test]
fn explicit_fields_retain_authoring_and_inert_event_records() {
    let (clip, _) = load(f::explicit_replay(), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    for field in [
        ClipField::Position,
        ClipField::Euler,
        ClipField::Float,
        ClipField::PPtr,
        ClipField::Editor,
        ClipField::EulerEditor,
    ] {
        assert!(
            matches!(clip.field(field, &meter).unwrap(), Some(Value::List(rows)) if rows.len() == 1)
        );
    }
    let Value::List(events) = clip.field(ClipField::Events, &meter).unwrap().unwrap() else {
        panic!("event dicts");
    };
    let parsed = clip.serialized();
    let event = events[0].as_dict().unwrap();
    assert_eq!(
        parsed.field(event, b"functionName").unwrap().as_str(),
        Some("fixture_marker")
    );
    assert_eq!(
        parsed.field(event, b"intParameter").unwrap().as_i64(),
        Some(-9)
    );
    let pointer = parsed
        .field(event, b"objectReferenceParameter")
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(
        parsed.field(pointer, b"m_PathID").unwrap().as_i64(),
        Some(-10)
    );
    assert_eq!(clip.timing().unwrap().unwrap().stop(), 6.);
}

fn rotation_fixture(packed: Option<bool>, compressed: bool) -> unityrs_testkit::ngui::Object {
    let identity = [0., 0., 0., 1.];
    let mut left = f::Key::new(0., identity);
    left.outgoing = [2., 0., 0., 0.];
    let right = f::Key::new(2., [0., 0., 0., -0.5]);
    let quat = f::curve("", &[left, right], [2; 2], Some(4), true, None);
    let bad_euler = f::curve(
        "",
        &[f::Key::new(0., [0.; 3])],
        [2; 2],
        Some(11),
        true,
        None,
    );
    let mut clip = f::explicit(
        9,
        4.,
        &[
            ("m_RotationCurves", vec![quat]),
            ("m_EulerCurves", vec![bad_euler]),
        ],
    );
    set(&mut clip.value, "m_Compressed", V::Bool(compressed));
    if let Some(slopes) = packed {
        set(
            &mut clip.value,
            "m_CompressedRotationCurves",
            f::one_row(f::packed_row(slopes)),
        );
    }
    clip
}

#[test]
fn quaternion_derivatives_stay_in_the_serialized_hemisphere() {
    let (clip, graph) = load(rotation_fixture(Some(true), false), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks()[0].source.field, ClipField::Rotation);
    let result = sample_clip(&bound, &rest(&graph), &[1., 2.], &meter).unwrap();
    let [x, y, z, w] = result.transforms()[0].pose.rotation;
    let norm = 0.5_f64.hypot(0.25);
    assert!((x - 0.5 / norm).abs() <= 2. * f64::EPSILON);
    assert!((w - 0.25 / norm).abs() <= 2. * f64::EPSILON);
    assert_eq!([y, z], [0., 0.]);
    assert_eq!(result.transforms()[1].pose.rotation, [0., 0., 0., -1.]);
}

#[test]
fn packed_times_values_and_slopes_are_distinct_and_the_selected_writer_refuses() {
    let meter = Meter::unlimited();
    for slopes in [true, false] {
        let (clip, graph) = load(rotation_fixture(Some(slopes), true), false);
        let clip = read_clip(&clip, &meter).unwrap();
        let packed = clip.packed_rotation(0, &meter).unwrap();
        assert_eq!(packed.times(), &[0.5, 1.75]);
        assert_eq!(packed.slopes(), slopes.then_some(&[-4., 0., 8.][..]));
        let [[x, y, z, w], [x2, y2, z2, w2]] = packed.values() else {
            panic!("two XYZW values");
        };
        assert_eq!([x, y, z], [x2, y2, z2]);
        assert!(*w > 0.99);
        assert_eq!(*w, -*w2);
        assert!(matches!(
            bind(&clip, &graph, &meter),
            Err(AnimError::UnsupportedPackedRotationSlope)
        ));
    }
    let (clip, graph) = load(rotation_fixture(None, true), false);
    let clip = read_clip(&clip, &meter).unwrap();
    assert_eq!(
        bind(&clip, &graph, &meter).unwrap().tracks()[0]
            .source
            .field,
        ClipField::Rotation
    );
}

#[test]
fn clamp_is_per_side_and_only_consulted_outside_the_curve() {
    let scale = |wrap| {
        f::curve(
            "",
            &[f::Key::new(3., [1., 2., 3.])],
            wrap,
            Some(4),
            true,
            None,
        )
    };
    let meter = Meter::unlimited();
    for (wrap, bad_time) in [([4, 2], 2.), ([2, 4], 6.)] {
        let mut object = f::explicit_fixture();
        set(&mut object.value, "m_ScaleCurves", f::one_row(scale(wrap)));
        let (clip, graph) = load(object, false);
        let clip = read_clip(&clip, &meter).unwrap();
        let bound = bind(&clip, &graph, &meter).unwrap();
        assert_eq!(
            sample_clip(&bound, &rest(&graph), &[3.], &meter)
                .unwrap()
                .transforms()[0]
                .pose
                .scale,
            [1., 2., 3.]
        );
        assert!(matches!(
            sample_clip(&bound, &rest(&graph), &[bad_time], &meter),
            Err(AnimError::UnsupportedCurveWrap(4))
        ));
    }
}

#[test]
fn a_scalar_transform_curve_preserves_unwritten_rest_components() {
    let row = f::curve(
        "",
        &[f::Key::new(2., [12.])],
        [2; 2],
        Some(4),
        true,
        Some(("m_LocalScale.y", 4)),
    );
    let (clip, graph) = load(f::explicit(9, 2., &[("m_FloatCurves", vec![row])]), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let pose = sample_clip(&bound, &rest(&graph), &[2.], &meter)
        .unwrap()
        .transforms()[0]
        .pose;
    assert_eq!(pose.scale, [1., 12., 1.]);
}

#[test]
fn chosen_invalid_keys_and_rotation_order_are_refused() {
    enum Reason {
        Order,
        NaN,
        Weights,
        Times,
    }
    for reason in [Reason::Order, Reason::NaN, Reason::Weights, Reason::Times] {
        let mut key = f::Key::new(0., [0.; 3]);
        match reason {
            Reason::NaN => key.outgoing[1] = f32::NAN,
            Reason::Weights => {
                key.weighted = 2;
                key.out_weight[0] = 1.25;
            }
            _ => {}
        }
        let right = f::Key::new(
            if matches!(reason, Reason::Times) {
                0.
            } else {
                1.
            },
            [0.; 3],
        );
        let order = if matches!(reason, Reason::Order) {
            7
        } else {
            4
        };
        let row = f::curve("", &[key, right], [2; 2], Some(order), true, None);
        let (clip, graph) = load(f::explicit(9, 2., &[("m_EulerCurves", vec![row])]), false);
        let meter = Meter::unlimited();
        let clip = read_clip(&clip, &meter).unwrap();
        let error = bind(&clip, &graph, &meter).unwrap_err();
        assert!(
            match reason {
                Reason::Order => matches!(error, AnimError::UnsupportedRotationOrder(Some(7))),
                Reason::NaN | Reason::Weights => matches!(error, AnimError::UnsupportedTangentMode),
                Reason::Times => matches!(error, AnimError::InvalidCurveKeys),
            },
            "{error}"
        );
    }
}

#[test]
fn stored_explicit_interval_overrides_key_extent_and_legacy_uses_explicit_source() {
    let mut object = f::compiled_fixture();
    set(&mut object.value, "m_Legacy", V::Bool(true));
    set(
        &mut object.value,
        "m_AnimationClipSettings",
        st(
            "AnimationClipSettings",
            vec![("m_StartTime", V::Float(1.)), ("m_StopTime", V::Float(7.))],
        ),
    );
    let (clip, graph) = load(object, false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks().len(), 1);
    let timing = bound.timing().unwrap();
    assert_eq!((timing.start(), timing.stop()), (1., 7.));
    let result = sample_clip(&bound, &rest(&graph), &[1., 7.], &meter).unwrap();
    assert_eq!(result.transforms()[0].pose.position, [2., -2., 3.]);
    assert_eq!(result.transforms()[1].pose.position, [10., 8., 4.]);
}

#[test]
fn equal_rank_rotation_writers_and_zero_sampled_quaternions_refuse() {
    let meter = Meter::unlimited();
    let curve = f::curve(
        "",
        &[
            f::Key::new(0., [0., 0., 0., 1.]),
            f::Key::new(2., [0., 0., 0., -1.]),
        ],
        [2; 2],
        Some(4),
        true,
        None,
    );
    let cases = [
        (vec![curve.clone(), curve.clone()], true),
        (vec![curve], false),
    ];
    for (rows, duplicate) in cases {
        let (clip, graph) = load(f::explicit(9, 4., &[("m_RotationCurves", rows)]), false);
        let clip = read_clip(&clip, &meter).unwrap();
        let bound = bind(&clip, &graph, &meter);
        if duplicate {
            assert!(matches!(bound, Err(AnimError::AmbiguousPlaybackSource)));
        } else {
            assert!(matches!(
                sample_clip(&bound.unwrap(), &rest(&graph), &[1.], &meter),
                Err(AnimError::InvalidQuaternion)
            ));
        }
    }
}

#[test]
fn editor_binding_table_without_muscle_storage_stays_metadata() {
    let mut object = f::compiled_fixture();
    let V::Struct(_, root) = &mut object.value else {
        panic!("clip");
    };
    root.retain(|(name, _)| name != "m_MuscleClip");
    set(
        &mut object.value,
        "m_AnimationClipSettings",
        st(
            "AnimationClipSettings",
            vec![("m_StartTime", V::Float(2.)), ("m_StopTime", V::Float(6.))],
        ),
    );
    let (clip, graph) = load(object, false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert_eq!(clip.bindings().unwrap().bindings().len(), 2);
    assert_eq!(
        bind(&clip, &graph, &meter).unwrap().tracks()[0]
            .source
            .field,
        ClipField::Position
    );
}

#[test]
fn a_nonroot_writer_fills_its_pose_from_that_nodes_rest() {
    let row = f::curve(
        "Child",
        &[f::Key::new(2., [2., 4., 8.])],
        [2; 2],
        Some(4),
        true,
        None,
    );
    let (clip, graph) = load(
        f::explicit(9, 2., &[("m_PositionCurves", vec![row])]),
        false,
    );
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let mut rest = rest(&graph);
    rest[1].scale = [3., 4., 5.];
    let samples = sample_clip(&bound, &rest, &[2.], &meter).unwrap();
    let [row] = samples.transforms() else {
        panic!("one animated child");
    };
    assert_eq!(row.node, 1);
    assert_eq!(row.pose.position, [2., 4., 8.]);
    assert_eq!(row.pose.scale, [3., 4., 5.]);
}
