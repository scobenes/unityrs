#![allow(
    clippy::float_cmp,
    reason = "assert exact stored values and key boundaries"
)]

#[path = "clip/built.rs"]
mod built;
#[path = "clip/compiled.rs"]
mod compiled;
#[path = "clip/explicit.rs"]
mod explicit;
#[path = "clip/replay.rs"]
mod replay;
mod support;

use support::{bind, load};
use unityrs_anim::{AnimError, ClipField, read_clip, sample_clip};
use unityrs_core::limits::Meter;
use unityrs_testkit::scene3d::anim::playback as f;

#[test]
fn explicit_key_bounds_and_local_time() {
    let (clip, graph) = load(f::explicit_fixture(), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let timing = bound.timing().unwrap();
    assert_eq!((timing.start(), timing.stop()), (2., 6.));
    assert_eq!(bound.tracks()[0].source.field, ClipField::Position);
    let samples = sample_clip(&bound, &support::rest(&graph), &[2., 4., 6.], &meter).unwrap();
    assert_eq!(samples.times()[1].local, 2.);
    assert_eq!(samples.transforms()[1].pose.position[0], 10.);
}

#[test]
fn compiled_clock_and_precedence() {
    let (clip, graph) = load(f::compiled_fixture(), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    let timing = bound.timing().unwrap();
    assert_eq!((timing.start(), timing.stop()), (2., 6.));
    assert!(
        bound
            .tracks()
            .iter()
            .all(|t| t.source.field == ClipField::Compiled)
    );
    let samples = sample_clip(&bound, &support::rest(&graph), &[2., 4., 6.], &meter).unwrap();
    assert_eq!(samples.transforms()[0].pose.position[0], 4.);
    assert_eq!(samples.transforms()[1].pose.position[0], 8.);
    assert_eq!(samples.transforms()[2].pose.position[0], -2.);
    assert_eq!(samples.transforms()[1].pose.scale, [2., 3., 4.]);
}

#[test]
fn an_explicit_schedule_cannot_reorder_or_leave_the_interval() {
    let (clip, graph) = load(f::explicit_fixture(), false);
    let meter = Meter::unlimited();
    let clip = read_clip(&clip, &meter).unwrap();
    let bound = bind(&clip, &graph, &meter).unwrap();
    for times in [&[][..], &[4., 4.], &[6., 2.], &[1.], &[7.], &[f64::NAN]] {
        assert!(matches!(
            sample_clip(&bound, &support::rest(&graph), times, &meter),
            Err(AnimError::InvalidSchedule)
        ));
    }
}

#[test]
fn stored_rate_is_required_even_for_explicit_sample_times() {
    use unityrs_testkit::{ngui::V, scene3d::set};
    for rate in [f32::NAN, f32::INFINITY, -1., 0.] {
        let mut object = f::explicit_fixture();
        set(&mut object.value, "m_SampleRate", V::Float(rate));
        let (clip, graph) = load(object, false);
        let meter = Meter::unlimited();
        let clip = read_clip(&clip, &meter).unwrap();
        assert!(matches!(
            bind(&clip, &graph, &meter),
            Err(AnimError::InvalidSampleRate)
        ));
    }
}
