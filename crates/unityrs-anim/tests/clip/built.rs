//! Compiled clips shaped like built 2022.3 generic clips: every streamed slot
//! is seeded, an Euler row's custom type is its rotation order, and the
//! streamed range holds the continuous and discrete slots. The values are
//! synthetic.
use crate::support::{bind, load, rest};
use unityrs_anim::{AnimError, SlotCounts, SlotValue, read_clip, sample_clip};
use unityrs_core::{env::PPtr, limits::Meter};
use unityrs_testkit::scene3d::{
    Layout,
    anim::{Binding, playback as f},
};

type Entry = (u32, [u32; 4]);

struct Layout2022<'a> {
    curves: u32,
    discrete: u32,
    dense: f::Dense<'a>,
    constants: &'a [u32],
}

impl Layout2022<'_> {
    fn streamed(curves: u32, discrete: u32) -> Self {
        Layout2022 {
            curves,
            discrete,
            dense: f::Dense::default(),
            constants: &[],
        }
    }
}

fn built(
    bindings: &[Binding],
    words: &[u32],
    layout: &Layout2022<'_>,
) -> unityrs_testkit::ngui::Object {
    let mut clip = f::explicit(9, 2., &[]);
    f::add_compiled(
        &mut clip,
        Layout::Unity2022,
        bindings,
        &[(0, -6), (1, 5)],
        f::Storage {
            interval: [2., 6.],
            streamed_count: layout.curves,
            words,
            dense: layout.dense,
            constants: layout.constants,
            discrete_count: Some(layout.discrete),
        },
    );
    clip
}

fn transform(attribute: u32, custom: u8) -> Binding {
    Binding {
        attribute,
        class: 4,
        custom,
        ..Binding::default()
    }
}

/// The `m_Mesh` binding of a `MeshFilter`, as a built clip stores it.
fn mesh_filter() -> Binding {
    Binding {
        class: 33,
        attribute: 3_757_223_753,
        custom: 41,
        pptr: 1,
        ..Binding::default()
    }
}

fn integer() -> Binding {
    Binding {
        class: 114,
        integer: 1,
        ..Binding::default()
    }
}

fn held(value: f32) -> [u32; 4] {
    [0, 0, 0, value.to_bits()]
}

/// A seed frame, then keys at each time, then the terminal frame.
fn stream(seed: &[Entry], keys: &[(f32, &[Entry])]) -> Vec<u32> {
    let mut words = Vec::new();
    f::frame(&mut words, f32::MIN, seed);
    for (time, slots) in keys {
        f::frame(&mut words, *time, slots);
    }
    f::frame(&mut words, f32::INFINITY, &[]);
    words
}

fn vector(first: u32, values: [f32; 3]) -> Vec<Entry> {
    (0..3)
        .map(|i| (first + i, held(values[i as usize])))
        .collect()
}

fn positions(words: &[u32], times: &[f64]) -> Result<Vec<[f64; 3]>, AnimError> {
    let (object, graph) = load(
        built(&[transform(1, 0)], words, &Layout2022::streamed(3, 0)),
        false,
    );
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter)?;
    let bound = bind(&clip, &graph, &meter)?;
    let samples = sample_clip(&bound, &rest(&graph), times, &meter)?;
    Ok(samples
        .transforms()
        .iter()
        .map(|row| row.pose.position)
        .collect())
}

#[test]
fn seeds_hold_their_constant_term_before_the_first_key() {
    // The built pattern: each seed repeats its first key with zero slopes.
    let first = vector(0, [1., 2., 3.]);
    let words = stream(&first, &[(2., &first), (6., &vector(0, [4., 5., 6.]))]);
    let rows = positions(&words, &[2., 4., 6.]).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows, [[1., 2., 3.], [1., 2., 3.], [4., 5., 6.]]);

    // Sloped and signed-zero seeds are accepted. A cubic from -FLT_MAX has no
    // finite value, so its constant term is held up to the first key, and a
    // seed with no later key holds over the whole interval.
    let seed = [
        (
            0,
            [
                1_f32.to_bits(),
                2_f32.to_bits(),
                3_f32.to_bits(),
                9_f32.to_bits(),
            ],
        ),
        (1, held(-0.)),
        (2, held(5.)),
    ];
    let key = [(0, held(1.)), (1, held(0.))];
    let words = stream(&seed, &[(3., &key), (6., &key)]);
    let rows = positions(&words, &[2., 2.5, 3., 6.]).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[..2], [[9., -0., 5.]; 2]);
    assert!(rows[0][1].is_sign_negative());
    assert_eq!(rows[2..], [[1., 0., 5.]; 2]);

    // Refuse a non-finite seed coefficient, a slot outside the stream and a
    // repeated seed slot.
    let mut nan = first.clone();
    nan[1].1[1] = f32::NAN.to_bits();
    let mut outside = first.clone();
    outside.push((3, held(0.)));
    let mut repeated = first.clone();
    repeated.push((0, held(1.)));
    for seed in [nan, outside, repeated] {
        let words = stream(&seed, &[(2., &first), (6., &first)]);
        assert!(matches!(
            positions(&words, &[2.]),
            Err(AnimError::InvalidStreamFrame)
        ));
    }
}

/// Independent reference: compose active right-handed axis rotation
/// matrices in application order, then convert the matrix to a quaternion.
fn reference(order: [usize; 3], degrees: [f64; 3]) -> [f64; 4] {
    let axis = |i: usize, angle: f64| {
        let (sine, cosine) = angle.to_radians().sin_cos();
        let (next, last) = ((i + 1) % 3, (i + 2) % 3);
        let mut matrix = [[0.; 3]; 3];
        matrix[i][i] = 1.;
        matrix[next][next] = cosine;
        matrix[last][last] = cosine;
        matrix[last][next] = sine;
        matrix[next][last] = -sine;
        matrix
    };
    let product = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut m = [[0.; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|n| a[r][n] * b[n][c]).sum();
            }
        }
        m
    };
    let mut m = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    for i in order {
        m = product(axis(i, degrees[i]), m);
    }
    // The trace is well above -1 for the angles used below.
    let w = (1. + m[0][0] + m[1][1] + m[2][2]).sqrt() / 2.;
    [
        (m[2][1] - m[1][2]) / (4. * w),
        (m[0][2] - m[2][0]) / (4. * w),
        (m[1][0] - m[0][1]) / (4. * w),
        w,
    ]
}

#[test]
fn compiled_euler_custom_type_is_the_rotation_order() {
    let degrees: [f32; 3] = [30., 50., 70.];
    let angles = vector(0, degrees);
    let words = stream(&angles, &[(2., &angles), (6., &angles)]);
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 2, 0],
        [1, 0, 2],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let meter = Meter::unlimited();
    let mut seen = Vec::new();
    for (custom, order) in (0..).zip(orders) {
        let layout = Layout2022::streamed(3, 0);
        let (object, graph) = load(built(&[transform(4, custom)], &words, &layout), false);
        let clip = read_clip(&object, &meter).unwrap();
        let bound = bind(&clip, &graph, &meter).unwrap();
        let samples = sample_clip(&bound, &rest(&graph), &[2., 4.], &meter).unwrap();
        assert_eq!(samples.transforms().len(), 2);
        let want = reference(order, degrees.map(f64::from));
        for row in samples.transforms() {
            let got = row.pose.rotation;
            let dot: f64 = got.iter().zip(want).map(|(a, b)| a * b).sum();
            assert!(
                (dot.abs() - 1.).abs() < 1e-12,
                "order {custom}: {got:?} != {want:?}"
            );
        }
        seen.push(want);
    }
    // Each order gives a distinct rotation for these angles.
    for (i, a) in seen.iter().enumerate() {
        for b in &seen[i + 1..] {
            let dot: f64 = a.iter().zip(b).map(|(a, b)| a * b).sum();
            assert!(dot.abs() < 1. - 1e-6);
        }
    }
    let (object, graph) = load(
        built(&[transform(4, 6)], &words, &Layout2022::streamed(3, 0)),
        false,
    );
    let clip = read_clip(&object, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnknownBindingWidth { custom_type: 6, .. })
    ));
}

fn pointer(file_id: i32, path_id: i64) -> SlotValue {
    SlotValue::Object(PPtr { file_id, path_id })
}

#[test]
fn several_discrete_curves_share_the_stream_in_binding_order() {
    // Slot 0 is the mesh, 1..3 the translation on the same path, 4 an integer.
    let mut seed = vec![(0, held(0.))];
    seed.extend(vector(1, [1., 2., 3.]));
    seed.push((4, [0, 0, 0, 7]));
    let translation = vector(1, [1., 2., 3.]);
    let words = stream(
        &seed,
        &[
            (2., &translation),
            (4., &[(0, held(1.))]),
            (5., &[(4, [0, 0, 0, 9])]),
            (6., &translation),
        ],
    );
    let bindings = [mesh_filter(), transform(1, 0), integer()];
    let (object, graph) = load(built(&bindings, &words, &Layout2022::streamed(3, 2)), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter).unwrap();
    let compiled = clip.compiled().unwrap().unwrap();
    assert_eq!(
        compiled.counts(),
        SlotCounts {
            streamed: 5,
            dense: 0,
            constant: 0
        }
    );
    let rows = [
        (0, 2., pointer(0, -6)),
        (0, 3.99, pointer(0, -6)),
        (0, 4., pointer(1, 5)),
        (4, 4.99, SlotValue::Integer(7)),
        (4, 5., SlotValue::Integer(9)),
    ];
    for (slot, time, value) in rows {
        assert_eq!(compiled.slot_value(slot, time, &meter).unwrap(), value);
    }
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks().len(), 1);
    let samples = sample_clip(&bound, &rest(&graph), &[2., 6.], &meter).unwrap();
    assert_eq!(samples.transforms().len(), 2);
    assert!(
        samples
            .transforms()
            .iter()
            .all(|row| row.pose.position == [1., 2., 3.])
    );

    // The count must match the discrete bindings in the streamed range.
    let (object, _) = load(built(&bindings, &words, &Layout2022::streamed(2, 3)), false);
    let clip = read_clip(&object, &meter).unwrap();
    assert!(matches!(
        clip.compiled(),
        Err(AnimError::UnsupportedClipLayout)
    ));
}

#[test]
fn dense_and_constant_slots_follow_the_split_stream() {
    let mut seed = vector(0, [1., 2., 3.]);
    seed.push((3, held(0.)));
    let translation = vector(0, [1., 2., 3.]);
    let words = stream(&seed, &[(2., &translation), (6., &translation)]);
    let scale = [1., 1., 1., 2., 3., 4.].map(f32::to_bits);
    let rotation = [0., 0., 0., 1.].map(f32::to_bits);
    let layout = Layout2022 {
        curves: 3,
        discrete: 1,
        dense: f::Dense {
            frames: 2,
            curves: 3,
            rate: 4.,
            begin: 2.,
            bits: &scale,
        },
        constants: &rotation,
    };
    let bindings = [
        transform(1, 0),
        mesh_filter(),
        transform(3, 0),
        transform(2, 0),
    ];
    let (object, graph) = load(built(&bindings, &words, &layout), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter).unwrap();
    let compiled = clip.compiled().unwrap().unwrap();
    assert_eq!(
        compiled
            .slot_bindings()
            .iter()
            .map(|slot| slot.first)
            .collect::<Vec<_>>(),
        [0, 3, 4, 7]
    );
    assert_eq!(compiled.slot_value(3, 2., &meter).unwrap(), pointer(0, -6));
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks().len(), 3);
    let samples = sample_clip(&bound, &rest(&graph), &[2., 6.], &meter).unwrap();
    let rows = samples
        .transforms()
        .iter()
        .map(|row| (row.pose.position, row.pose.scale, row.pose.rotation))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ([1., 2., 3.], [1., 1., 1.], [0., 0., 0., 1.]));
    assert_eq!(rows[1], ([1., 2., 3.], [2., 3., 4.], [0., 0., 0., 1.]));
}
