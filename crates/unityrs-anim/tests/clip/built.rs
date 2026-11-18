//! Compiled clips shaped like built 2022.3 generic clips: every streamed slot
//! is seeded, Euler rows use custom type 4, and discrete slots follow the
//! continuous ones. The values are synthetic.
use crate::support::{bind, load, rest};
use unityrs_anim::{AnimError, SlotCounts, SlotValue, read_clip, sample_clip};
use unityrs_core::{env::PPtr, limits::Meter};
use unityrs_testkit::scene3d::{
    Layout,
    anim::{Binding, playback as f},
};

fn built(
    bindings: &[Binding],
    words: &[u32],
    curves: u32,
    discrete: u32,
    constants: &[u32],
) -> unityrs_testkit::ngui::Object {
    let mut clip = f::explicit(9, 2., &[]);
    f::add_compiled(
        &mut clip,
        Layout::Unity2022,
        bindings,
        &[(0, -6), (1, 5)],
        f::Storage {
            interval: [2., 6.],
            streamed_count: curves,
            words,
            constants,
            discrete_count: Some(discrete),
            ..f::Storage::default()
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

fn held(value: f32) -> [u32; 4] {
    [0, 0, 0, value.to_bits()]
}

/// A seed frame, then held keys at each time, then the terminal frame.
type Entry = (u32, [u32; 4]);

fn stream(seed: &[Entry], keys: &[(f32, &[Entry])]) -> Vec<u32> {
    let mut words = Vec::new();
    f::frame(&mut words, f32::MIN, seed);
    for (time, slots) in keys {
        f::frame(&mut words, *time, slots);
    }
    f::frame(&mut words, f32::INFINITY, &[]);
    words
}

fn translation(slots: [f32; 3]) -> Vec<Entry> {
    (0..3).map(|i| (i, held(slots[i as usize]))).collect()
}

#[test]
fn seeded_continuous_slots_hold_their_first_key() {
    let first = translation([1., 2., 3.]);
    let words = stream(&first, &[(2., &first), (6., &translation([4., 5., 6.]))]);
    let (object, graph) = load(built(&[transform(1, 0)], &words, 3, 0, &[]), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let samples = sample_clip(&bound, &rest(&graph), &[2., 4., 6.], &meter).unwrap();
    let positions = samples
        .transforms()
        .iter()
        .map(|row| row.pose.position)
        .collect::<Vec<_>>();
    assert_eq!(positions, [[1., 2., 3.], [1., 2., 3.], [4., 5., 6.]]);

    let mut sloped = first.clone();
    sloped[1].1[2] = 1_f32.to_bits();
    let mut disagreeing = first.clone();
    disagreeing[2].1 = held(7.);
    for seed in [sloped, disagreeing] {
        let words = stream(&seed, &[(2., &first), (6., &first)]);
        let (object, graph) = load(built(&[transform(1, 0)], &words, 3, 0, &[]), false);
        let clip = read_clip(&object, &meter).unwrap();
        assert!(matches!(
            bind(&clip, &graph, &meter),
            Err(AnimError::InvalidStreamFrame)
        ));
    }
}

#[test]
fn built_euler_rows_use_custom_type_four_and_zxy_order() {
    let zx = [(0, held(90.)), (1, held(0.)), (2, held(90.))];
    let xy = [(0, held(90.)), (1, held(90.)), (2, held(0.))];
    let words = stream(&xy, &[(2., &xy), (4., &zx), (6., &zx)]);
    let (object, graph) = load(built(&[transform(4, 4)], &words, 3, 0, &[]), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let samples = sample_clip(&bound, &rest(&graph), &[2., 4.], &meter).unwrap();
    // ZXY applies Z, then X, then Y: q = qY * qX * qZ.
    let expected = [[0.5, 0.5, -0.5, 0.5], [0.5, -0.5, 0.5, 0.5]];
    for (row, want) in samples.transforms().iter().zip(expected) {
        let got = row.pose.rotation;
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-12),
            "{got:?} != {want:?}"
        );
    }
    // Other Transform custom types remain unknown.
    let (object, graph) = load(built(&[transform(4, 5)], &words, 3, 0, &[]), false);
    let clip = read_clip(&object, &meter).unwrap();
    assert!(matches!(
        bind(&clip, &graph, &meter),
        Err(AnimError::UnknownBindingWidth { custom_type: 5, .. })
    ));
}

#[test]
fn discrete_slots_stream_after_the_continuous_slots() {
    let first = translation([1., 2., 3.]);
    let mut seed = first.clone();
    seed.push((3, held(0.)));
    let words = stream(&seed, &[(2., &first), (4., &[(3, held(1.))]), (6., &first)]);
    let bindings = [transform(1, 0), mesh_filter()];
    let (object, graph) = load(built(&bindings, &words, 3, 1, &[]), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&object, &meter).unwrap();
    let compiled = clip.compiled().unwrap().unwrap();
    assert_eq!(
        compiled.counts(),
        SlotCounts {
            streamed: 4,
            dense: 0,
            constant: 0
        }
    );
    let pointer = |file_id, path_id| SlotValue::Object(PPtr { file_id, path_id });
    for (time, value) in [
        (2., pointer(0, -6)),
        (3.99, pointer(0, -6)),
        (4., pointer(1, 5)),
    ] {
        assert_eq!(compiled.slot_value(3, time, &meter).unwrap(), value);
    }
    let bound = bind(&clip, &graph, &meter).unwrap();
    assert_eq!(bound.tracks().len(), 1);
    let samples = sample_clip(&bound, &rest(&graph), &[2., 6.], &meter).unwrap();
    assert!(
        samples
            .transforms()
            .iter()
            .all(|row| row.pose.position == [1., 2., 3.])
    );

    // A discrete count must cover exactly the trailing discrete bindings, and
    // its order against dense or constant slots is unconfirmed.
    let reversed = [mesh_filter(), transform(1, 0)];
    let constant = [transform(1, 0), mesh_filter(), transform(3, 0)];
    let ones = [1_f32.to_bits(); 3];
    for (bindings, constants) in [(&reversed[..], &[][..]), (&constant[..], &ones[..])] {
        let (object, _) = load(built(bindings, &words, 3, 1, constants), false);
        let clip = read_clip(&object, &meter).unwrap();
        assert!(matches!(
            clip.compiled(),
            Err(AnimError::UnsupportedClipLayout)
        ));
    }
}
