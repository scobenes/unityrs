use crate::support::{bind, load, rest};
use unityrs_anim::{AnimError, ClipField, SlotCounts, SlotValue, read_clip, sample_clip};
use unityrs_core::{env::PPtr, limits::Meter};
use unityrs_testkit::{
    ngui::{V, vector},
    scene3d::{
        Layout,
        anim::{Binding, playback as f},
        set,
    },
};

fn object(bindings: &[Binding], storage: f::Storage<'_>) -> unityrs_testkit::ngui::Object {
    let mut clip = f::explicit(9, 2., &[]);
    f::add_compiled(
        &mut clip,
        Layout::Unity2022,
        bindings,
        &[(0, -6), (1, 5)],
        storage,
    );
    clip
}

fn transform(attribute: u32) -> Binding {
    Binding {
        attribute,
        class: 4,
        ..Binding::default()
    }
}

#[test]
fn compiled_pointer_seed_and_integer_float_bits_are_distinct() {
    let pointer = Binding {
        class: 137,
        pptr: 1,
        ..Binding::default()
    };
    let integer = Binding {
        class: 114,
        integer: 1,
        ..Binding::default()
    };
    let material = Binding {
        class: 137,
        custom: 22,
        ..Binding::default()
    };
    let mut words = Vec::new();
    f::frame(&mut words, f32::MIN, &[(0, [0; 4])]);
    f::frame(&mut words, 3., &[(0, [0, 0, 0, 1_f32.to_bits()])]);
    f::frame(&mut words, f32::INFINITY, &[]);
    let integer_bits = 0x7f80_0001;
    let storage = f::Storage {
        interval: [2., 6.],
        streamed_count: 1,
        words: &words,
        dense: f::Dense {
            frames: 2,
            curves: 1,
            rate: 4.,
            begin: 3.,
            bits: &[integer_bits, u32::MAX],
        },
        constants: &[12, 4, 5, 6].map(|x: u16| f32::from(x).to_bits()),
        discrete_count: None,
    };
    let (clip, graph) = load(
        object(&[pointer, integer, material, transform(1)], storage),
        false,
    );
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let compiled = clip.compiled().unwrap().unwrap();
    assert_eq!(
        compiled.counts(),
        SlotCounts {
            streamed: 1,
            dense: 1,
            constant: 4
        }
    );
    assert_eq!(
        compiled
            .slot_bindings()
            .iter()
            .map(|s| s.first)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert_eq!(
        compiled.streamed()[0].seed(),
        Some(PPtr {
            file_id: 0,
            path_id: -6
        })
    );
    assert_eq!(compiled.streamed()[0].keys().len(), 1);
    assert_eq!(compiled.streamed()[0].keys()[0].time, 3.);
    assert_eq!(
        compiled.slot_value(0, 2.99, &meter).unwrap(),
        SlotValue::Object(PPtr {
            file_id: 0,
            path_id: -6
        })
    );
    assert_eq!(
        compiled.slot_value(0, 3., &meter).unwrap(),
        SlotValue::Object(PPtr {
            file_id: 1,
            path_id: 5
        })
    );
    for (time, value) in [(2., 0x7f80_0001), (3.125, 0x7f80_0001), (3.25, -1)] {
        assert_eq!(
            compiled.slot_value(1, time, &meter).unwrap(),
            SlotValue::Integer(value)
        );
    }
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks().len(), 1);
    assert_eq!(bound.tracks()[0].source.ordinal, 3);
    let result = sample_clip(&bound, &rest(&graph), &[2., 4., 6.], &meter).unwrap();
    assert!(
        result
            .transforms()
            .iter()
            .all(|row| row.pose.position == [4., 5., 6.])
    );
}

#[test]
fn constant_only_uses_serialized_endpoints_and_zero_duration_schedules_once() {
    for stop in [4., 5.1] {
        let (clip, graph) = load(
            object(
                &[transform(3)],
                f::Storage {
                    interval: [4., stop],
                    constants: &[2_f32, 3., 4.].map(f32::to_bits),
                    ..f::Storage::default()
                },
            ),
            false,
        );
        let meter = Meter::unlimited();
        let clip = read_clip(&clip, &meter).unwrap();
        let bound = bind(&clip, &graph, &meter).unwrap();
        let timing = bound.timing().unwrap();
        let schedule = timing.schedule(None, &meter).unwrap();
        if stop == 4. {
            assert_eq!(schedule.as_slice(), &[4.]);
        } else {
            assert_eq!(schedule.as_slice(), &[4., 4.5, 5., f64::from(stop)]);
        }
        let result = sample_clip(&bound, &rest(&graph), schedule.as_slice(), &meter).unwrap();
        assert!(
            result
                .transforms()
                .iter()
                .all(|row| row.pose.scale == [2., 3., 4.])
        );
    }
}

#[test]
fn unknown_width_mismatched_counts_and_mixed_transform_storage_refuse() {
    let meter = Meter::unlimited();
    let storage = f::Storage {
        interval: [2., 6.],
        constants: &[1_f32.to_bits(); 2],
        ..f::Storage::default()
    };
    let (clip, graph) = load(
        object(
            &[
                Binding {
                    class: 114,
                    custom: 250,
                    ..Binding::default()
                },
                transform(1),
            ],
            storage,
        ),
        false,
    );
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnknownBindingWidth { binding: 0, .. })
    ));
    let (clip, graph) = load(object(&[transform(1)], storage), false);
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::SlotCount {
            bindings: 3,
            storage: 2
        })
    ));
    let (clip, graph) = load(
        object(
            &[transform(1)],
            f::Storage {
                dense: f::Dense {
                    frames: 1,
                    curves: 1,
                    rate: 5.,
                    begin: 3.,
                    bits: &[1_f32.to_bits()],
                },
                ..storage
            },
        ),
        false,
    );
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnsupportedMixedTransformStorage(0))
    ));
}

#[test]
fn bad_compiled_source_refuses_even_with_a_valid_explicit_fallback() {
    let meter = Meter::unlimited();
    let mut clip = f::explicit_fixture();
    f::add_compiled(
        &mut clip,
        Layout::Unity2022,
        &[transform(1)],
        &[],
        f::Storage {
            interval: [2., 6.],
            constants: &[1_f32.to_bits(), f32::INFINITY.to_bits(), 3_f32.to_bits()],
            ..f::Storage::default()
        },
    );
    let (clip, graph) = load(clip, false);
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(clip.field(ClipField::Position, &meter).unwrap().is_some());
    assert!(matches!(clip.issue(), Some(AnimError::InvalidSampleValue)));
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::InvalidSampleValue)
    ));
}

#[test]
fn extra_explicit_properties_require_supplemental_playback_evidence() {
    let mut clip = f::compiled_fixture();
    let row = f::curve(
        "",
        &[f::Key::new(2., [0., 0., 0., 1.])],
        [2; 2],
        Some(4),
        true,
        None,
    );
    set(&mut clip.value, "m_RotationCurves", f::one_row(row));
    let (clip, graph) = load(clip, false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnsupportedSupplementalCurve)
    ));
}

#[test]
fn float_rotation_refuses_only_without_a_compiled_rotation_owner() {
    let meter = Meter::unlimited();
    for attribute in [1, 2] {
        let mut clip = object(
            &[transform(attribute)],
            f::Storage {
                interval: [2., 6.],
                constants: &[0, 0, 1_f32.to_bits(), 0][..if attribute == 2 { 4 } else { 3 }],
                ..f::Storage::default()
            },
        );
        let row = f::curve(
            "",
            &[f::Key::new(2., [0.75])],
            [2; 2],
            Some(4),
            true,
            Some(("m_LocalRotation.x", 4)),
        );
        set(&mut clip.value, "m_FloatCurves", f::one_row(row));
        let (clip, graph) = load(clip, false);
        let clip = read_clip(&clip, &meter).unwrap();
        let bound = bind(&clip, &graph, &meter);
        if attribute == 2 {
            let bound = bound.unwrap();
            assert_eq!(bound.tracks().len(), 1);
            assert_eq!(bound.tracks()[0].source.field, ClipField::Compiled);
            let samples = sample_clip(&bound, &rest(&graph), &[2.], &meter).unwrap();
            assert_eq!(samples.transforms()[0].pose.rotation, [0., 0., 1., 0.]);
        } else {
            assert!(matches!(bound, Err(AnimError::UnsupportedBindingTarget)));
        }
    }
}

#[test]
fn stream_shape_and_slot_indices_are_validated_before_playback() {
    let known = [transform(1)];
    let meter = Meter::unlimited();
    let cases: &[&[u32]] = &[
        &[0, u32::MAX],
        &[0, 1, 3, 0, 0, 0, 0],
        &[f32::INFINITY.to_bits(), 1, 0, 0, 0, 0, 0],
        &[f32::INFINITY.to_bits(), 0, 0, 0],
    ];
    for &words in cases {
        let (clip, graph) = load(
            object(
                &known,
                f::Storage {
                    interval: [2., 6.],
                    streamed_count: 3,
                    words,
                    ..f::Storage::default()
                },
            ),
            false,
        );
        let clip = read_clip(&clip, &meter).unwrap();
        assert!(matches!(
            bind(&clip, &graph, &meter),
            Err(AnimError::InvalidStreamFrame)
        ));
    }
    let (clip, graph) = load(
        object(
            &known,
            f::Storage {
                interval: [2., 6.],
                streamed_count: 3,
                words: &[f32::INFINITY.to_bits(), 0],
                ..f::Storage::default()
            },
        ),
        false,
    );
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::MissingStreamKey(0))
    ));
}

#[test]
fn dense_interpolation_caps_a_rounded_fraction_at_the_next_value() {
    let rate = 29_f32 / 3.;
    let frame_time = |frame| 1. + f64::from(frame) / f64::from(rate);
    let sample = frame_time(4).next_down();
    assert!((sample - frame_time(3)) * f64::from(rate) > 1.);

    let mut bits = [0; 5 * 3];
    bits[4 * 3] = 1_f32.to_bits();
    let (clip, _) = load(
        object(
            &[transform(1)],
            f::Storage {
                interval: [1., 2.],
                dense: f::Dense {
                    frames: 5,
                    curves: 3,
                    rate,
                    begin: 1.,
                    bits: &bits,
                },
                ..f::Storage::default()
            },
        ),
        false,
    );
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert_eq!(
        clip.compiled()
            .unwrap()
            .unwrap()
            .slot_value(0, sample, &meter)
            .unwrap(),
        SlotValue::Float(1.)
    );
}

#[test]
fn bad_dense_dimensions_rate_and_interval_are_distinct() {
    let meter = Meter::unlimited();
    let base = f::Storage {
        interval: [2., 6.],
        dense: f::Dense {
            frames: 1,
            curves: 3,
            rate: 5.,
            begin: 2.,
            bits: &[1_f32.to_bits(); 3],
        },
        ..f::Storage::default()
    };
    for storage in [
        f::Storage {
            dense: f::Dense {
                frames: i32::MAX,
                ..base.dense
            },
            ..base
        },
        f::Storage {
            dense: f::Dense {
                rate: 0.,
                ..base.dense
            },
            ..base
        },
        f::Storage {
            dense: f::Dense {
                begin: f32::NAN,
                ..base.dense
            },
            ..base
        },
    ] {
        let (clip, _) = load(object(&[transform(1)], storage), false);
        let clip = read_clip(&clip, &meter).unwrap();
        assert!(matches!(clip.issue(), Some(AnimError::InvalidDenseGrid)));
    }
    let (clip, _) = load(
        object(
            &[transform(1)],
            f::Storage {
                interval: [6., 2.],
                ..base
            },
        ),
        false,
    );
    assert!(matches!(
        read_clip(&clip, &meter).unwrap().issue(),
        Some(AnimError::InvalidClipInterval)
    ));
}

#[test]
fn humanoid_slots_refuse_generic_playback() {
    let mut object = f::compiled_fixture();
    set_humanoid_mapping(&mut object);
    let (clip, _) = load(object, false);
    assert!(matches!(
        read_clip(&clip, &Meter::unlimited()).unwrap().issue(),
        Some(AnimError::UnsupportedHumanoidPlayback)
    ));
}

fn set_humanoid_mapping(object: &mut unityrs_testkit::ngui::Object) {
    let V::Struct(_, root) = &mut object.value else {
        panic!("clip");
    };
    let muscle = &mut root
        .iter_mut()
        .find(|(name, _)| name == "m_MuscleClip")
        .unwrap()
        .1;
    set(
        muscle,
        "m_IndexArray",
        vector(
            V::Int(-1),
            vec![V::Int(-1); 7].into_iter().chain([V::Int(0)]).collect(),
        ),
    );
}

#[test]
fn humanoid_mapping_refuses_even_with_empty_storage_and_explicit_curves() {
    let mut clip = f::explicit_fixture();
    f::add_compiled(
        &mut clip,
        Layout::Unity2022,
        &[],
        &[],
        f::Storage::default(),
    );
    set_humanoid_mapping(&mut clip);
    let (clip, graph) = load(clip, false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnsupportedHumanoidPlayback)
    ));
}

#[test]
fn discrete_index_requires_a_numeric_integral_in_bounds_mapping_index() {
    let pointer = Binding {
        class: 137,
        pptr: 1,
        ..Binding::default()
    };
    for index in [f32::NAN, -1., 0.5, 2., f32::INFINITY] {
        let (clip, _) = load(
            object(
                &[pointer],
                f::Storage {
                    interval: [2., 6.],
                    constants: &[index.to_bits()],
                    ..f::Storage::default()
                },
            ),
            false,
        );
        let clip = read_clip(&clip, &Meter::unlimited()).unwrap();
        assert!(matches!(clip.issue(), Some(AnimError::InvalidPPtrIndex(0))));
    }
}

#[test]
fn an_empty_compiled_table_with_a_terminal_marker_leaves_explicit_playback() {
    let mut object = f::explicit_fixture();
    f::add_compiled(
        &mut object,
        Layout::Unity2022,
        &[],
        &[],
        f::Storage {
            words: &[f32::INFINITY.to_bits(), 0],
            dense: f::Dense {
                frames: 1,
                ..f::Dense::default()
            },
            ..f::Storage::default()
        },
    );
    let (clip, graph) = load(object, false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(clip.compiled().unwrap().is_none());
    assert_eq!(
        bind(&clip, &graph, &meter).unwrap().tracks()[0]
            .source
            .field,
        ClipField::Position
    );
}

#[test]
fn metadata_only_slots_do_not_emit_pose_channels() {
    let integer = Binding {
        class: 114,
        integer: 1,
        ..Binding::default()
    };
    let (clip, graph) = load(
        object(
            &[integer],
            f::Storage {
                interval: [2., 6.],
                constants: &[u32::MAX],
                ..f::Storage::default()
            },
        ),
        false,
    );
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    assert!(clip.has_metadata_tracks());
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert!(bound.tracks().is_empty());
    let sampled = sample_clip(&bound, &[], &[], &meter).unwrap();
    assert_eq!(sampled.transforms(), []);
    assert_eq!(sampled.morphs(), []);
}
