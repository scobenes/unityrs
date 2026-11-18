use super::{
    ALL, ExportError, J, Key, Object, accessor, anim, animated, clip_input, doc, file, glb, input,
    p, s,
};

fn weighted_clip(weights: [f32; 2], slope: f32) -> Object {
    let mut first = Key::new(0., [0.; 3]);
    first.weighted = 2;
    first.out_weight = [weights[0]; 3];
    first.outgoing = [slope, 0., 0.];
    let mut last = Key::new(1., [1., 0., 0.]);
    last.weighted = 1;
    last.in_weight = [weights[1]; 3];
    last.incoming = [slope, 0., 0.];
    p::explicit(
        9,
        1.,
        &[(
            "m_PositionCurves",
            vec![p::curve("", &[first, last], [2, 2], None, true, None)],
        )],
    )
}

#[test]
fn linear_weighted_curve_bakes_with_a_flat_time_endpoint() {
    let (env, root, graph, meter) = clip_input(weighted_clip([0., 0.5], 1.));
    let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
    let bytes = glb(&collected.scene, &meter);
    let doc = doc(&bytes);
    let sampler = &doc["animations"][0]["samplers"][0];
    let times = accessor(&bytes, &doc, &sampler["input"]);
    let values = accessor(&bytes, &doc, &sampler["output"]);
    assert_eq!(times, [0., 0.25, 0.5, 0.75, 1.]);
    for (time, value) in times.iter().zip(values.as_chunks::<3>().0) {
        assert_eq!(*value, [-time, 0., 0.]);
    }
}

#[test]
fn weighted_curve_with_unbounded_source_rate_refuses_to_bake() {
    let (env, root, graph, meter) = clip_input(weighted_clip([1., 1.], 0.));
    assert!(matches!(
        animated(&env, &root, &graph, &meter, ALL),
        Err(ExportError::Bake("cannot certify subdivision"))
    ));
}

fn vector<const N: usize>(value: &J) -> [f64; N] {
    std::array::from_fn(|axis| value[axis].as_f64().unwrap())
}

#[test]
fn animated_child_composes_with_mirrored_parent_rest_trs() {
    let half_turn = std::f32::consts::FRAC_1_SQRT_2;
    let mut parent = s::transform((0, 1), (0, 0), &[(0, 5)], false);
    for (field, kind, value) in [
        ("m_LocalPosition", "Vector3f", &[10., 20., 30.][..]),
        (
            "m_LocalRotation",
            "Quaternionf",
            &[0., 0., half_turn, half_turn][..],
        ),
        ("m_LocalScale", "Vector3f", &[-2., 3., 4.][..]),
    ] {
        s::set(&mut parent, field, s::xyz(kind, value));
    }
    let mut first = Key::new(0., [1., 2., 3.]);
    first.outgoing = [1., 2., 3.];
    let mut last = Key::new(1., [2., 4., 6.]);
    last.incoming = first.outgoing;
    let clip = p::explicit(
        9,
        1.,
        &[(
            "m_PositionCurves",
            vec![p::curve("child", &[first, last], [2, 2], None, true, None)],
        )],
    );
    let (env, root, graph, meter) = input(
        file(&[
            Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 3)])),
            Object::new(2, 4, parent),
            anim::legacy(3, (0, 9), &[]),
            Object::new(4, 1, s::game_object(b"child", &[(0, 5)])),
            Object::new(5, 4, s::transform((0, 4), (0, 2), &[], false)),
            clip,
        ]),
        0,
    );
    let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
    let bytes = glb(&collected.scene, &meter);
    let doc = doc(&bytes);
    let animation = &doc["animations"][0];
    assert_eq!(doc["nodes"][0]["children"], serde_json::json!([1]));
    assert_eq!(
        animation["channels"][0]["target"],
        serde_json::json!({"node":1,"path":"translation"})
    );
    let sampler = &animation["samplers"][0];
    assert_eq!(sampler["interpolation"], "LINEAR");
    let times = accessor(&bytes, &doc, &sampler["input"]);
    let values = accessor(&bytes, &doc, &sampler["output"]);

    let time = 0.375;
    let right = times.partition_point(|&key| key < time);
    let left = right - 1;
    assert!(times[left] < time && time < times[right]);
    let fraction = (time - times[left]) / (times[right] - times[left]);
    let child: [f64; 3] = std::array::from_fn(|axis| {
        let start = values[left * 3 + axis];
        start + (values[right * 3 + axis] - start) * fraction
    });
    let [px, py, pz] = vector(&doc["nodes"][0]["translation"]);
    let [sx, sy, sz] = vector(&doc["nodes"][0]["scale"]);
    let [qx, qy, qz, qw] = vector(&doc["nodes"][0]["rotation"]);
    assert_eq!([qx, qy], [0., 0.]);
    let norm = qz * qz + qw * qw;
    let cos = (qw * qw - qz * qz) / norm;
    let sin = 2. * qz * qw / norm;
    let [x, y, z] = [sx * child[0], sy * child[1], sz * child[2]];
    let world = [px + cos * x - sin * y, py + sin * x + cos * y, pz + z];
    // In Unity, the parent's 90-degree turn sends its scaled X to +Y
    // and its scaled Y to -X. Reflect the resulting world point once.
    let expected = [-4. + 6. * time, 18. - 2. * time, 42. + 12. * time];
    for (actual, expected) in world.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-6, "{world:?}");
    }
}
