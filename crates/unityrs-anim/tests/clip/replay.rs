use crate::support;
use serde_json::Value;
use unityrs_anim::{Attachment, RendererChannel, RendererChannels, read_clip, sample_clip};
use unityrs_core::limits::Meter;
use unityrs_scene::Component;
use unityrs_testkit::scene3d::anim::playback as fixture;

const BOUNDS: &str = include_str!("../../../../tools/parity/anim_bounds.json");
const EXPECTED: &str = include_str!("../support/clip_expected.json");

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn array<const N: usize>(values: &Value) -> [f64; N] {
    let values = values.as_array().unwrap();
    assert_eq!(values.len(), N);
    std::array::from_fn(|i| number(&values[i]))
}

fn scalar_bound(row: &Value, limits: &Value) -> f64 {
    let equation = row["equation"].as_str().unwrap();
    if equation == "weighted" {
        // The generator uses exact rational brackets for the inverse-time
        // envelope. It records that term with the weighted arithmetic bound.
        return number(&row["bound"]);
    }
    let n = number(&limits["operations"][equation]);
    let scale = number(&row["scale"]);
    assert!(scale <= number(&limits["max_operation_scale"]));
    let roundoff = n * number(&limits["unit_roundoff"]);
    number(&limits["absolute_floor"]) + (roundoff / (1. - roundoff)) * scale
}

fn check_component(got: f64, row: &Value, limits: &Value, location: &str) {
    let expected = number(&row["value"]);
    let limit = 2. * scalar_bound(row, limits);
    assert!(
        (got - expected).abs() <= limit,
        "{location}: {got} vs {expected}, bound {limit}"
    );
}

fn angle(a: [f64; 4], mut b: [f64; 4]) -> f64 {
    fn norm(v: [f64; 4]) -> f64 {
        v.into_iter().map(|x| x * x).sum::<f64>().sqrt()
    }
    let a = a.map(|x| x / norm(a));
    b = b.map(|x| x / norm(b));
    if a.into_iter().zip(b).map(|(x, y)| x * y).sum::<f64>() < 0. {
        b = b.map(|x| -x);
    }
    let diff = norm(std::array::from_fn(|i| a[i] - b[i]));
    let sum = norm(std::array::from_fn(|i| a[i] + b[i]));
    4. * diff.atan2(sum)
}

#[test]
fn rust_replays_python_curves_with_frozen_f64_bounds() {
    let values: Value = serde_json::from_str(EXPECTED).unwrap();
    let frozen: Value = serde_json::from_str(BOUNDS).unwrap();
    assert_eq!(values["version"], frozen["version"]);
    let limits = &frozen["transcription"];
    let counts = [
        ("explicit", fixture::explicit_replay(), true, 6),
        ("streamed", fixture::compiled_fixture(), false, 6),
        ("dense", fixture::dense_replay(), true, 6),
    ];
    for (name, object, with_morph, count) in counts {
        let (clip, graph) = support::load(object, with_morph);
        let meter = Meter::unlimited();
        let clip = read_clip(&clip, &meter).unwrap();
        let channels = [RendererChannel {
            name: b"Lift",
            hash: 3_545_215_282,
            target_slot: 2,
            frame: 1,
            full_weight: 80.,
        }];
        let mut renderer_table = vec![];
        if with_morph {
            let renderer = graph.nodes()[0]
                .components
                .as_slice()
                .iter()
                .find_map(|component| {
                    if let Component::SkinnedMeshRenderer(renderer) = component {
                        Some(renderer)
                    } else {
                        None
                    }
                })
                .expect("skin fixture");
            renderer_table.push(RendererChannels {
                renderer: &renderer.renderer.object,
                attachment: Attachment::Present(&channels),
            });
        }
        let bound = support::bind_morph(&clip, &graph, &renderer_table, &meter).unwrap();
        let rows = values[name].as_array().unwrap();
        assert_eq!(rows.len(), count);
        let times: Vec<_> = rows.iter().map(|row| number(&row["source"])).collect();
        assert!(
            times
                .iter()
                .all(|t| t.abs() <= number(&limits["max_abs_source_time"]))
        );
        let rest = [unityrs_scene::Trs {
            position: [9., 8., 7.],
            rotation: [0., 0., 0., 1.],
            scale: [8., 9., 10.],
        }];
        let samples = sample_clip(&bound, &rest, &times, &meter).unwrap();
        assert_eq!(samples.transforms().len(), count);
        assert_eq!(samples.morphs().len(), if with_morph { count } else { 0 });
        for (i, row) in rows.iter().enumerate() {
            let pose = samples.transforms()[i].pose;
            let location = format!("{name}/{}", times[i]);
            assert_eq!(samples.times()[i].local, number(&row["local"]));
            for (value, expected) in pose
                .position
                .into_iter()
                .zip(row["position"].as_array().unwrap())
            {
                check_component(value, expected, limits, &location);
            }
            for (value, expected) in pose.scale.into_iter().zip(row["scale"].as_array().unwrap()) {
                check_component(value, expected, limits, &location);
            }
            let rotation = &row["rotation"];
            let actual_angle = angle(pose.rotation, array(&rotation["value"]));
            let min_norm = number(&limits["min_raw_quaternion_norm"]);
            let component_error = 2. * scalar_bound(rotation, limits);
            let arithmetic =
                number(&limits["angular_floor_u64"]) * number(&limits["unit_roundoff"]);
            let angle_limit = 4. * (2. * component_error / min_norm).min(1.).asin() + arithmetic;
            assert!(
                actual_angle <= angle_limit,
                "{location}: angle {actual_angle}, bound {angle_limit}"
            );
            if with_morph {
                assert_eq!(samples.morphs()[i].target_slot, 2);
                check_component(
                    samples.morphs()[i].weight,
                    &row["weight"],
                    limits,
                    &location,
                );
            }
        }
    }
}
