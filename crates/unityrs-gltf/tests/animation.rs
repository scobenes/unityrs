#![allow(clippy::float_cmp)]
use serde_json::json;
use unityrs_core::limits::Meter;
use unityrs_gltf::*;

mod support;

const ROTATION_SIGN_ERROR: &str = "LINEAR animation rotation signs must be continuous in f32";

#[test]
fn reconstructs_animated_instances_in_first_reach_order() {
    let scene = support::animated(&Meter::unlimited()).unwrap();
    let (bytes, _) = support::write(&scene, Options::default());
    let doc = support::json(&bytes);
    let animation = &doc["animations"][0];
    assert_eq!(doc["animations"].as_array().unwrap().len(), 2);
    assert_eq!(animation["name"], "sweep\"é");
    assert_eq!(doc["animations"][1]["name"], animation["name"]);
    assert_eq!(animation["extras"], json!({"source": "synthetic"}));
    assert_eq!(doc["nodes"][0]["mesh"], doc["nodes"][1]["mesh"]);
    assert_eq!(doc["nodes"][0]["weights"], json!([0.25, 0.5]));
    assert_eq!(doc["nodes"][1]["weights"], json!([0.75, 0.25]));
    assert_eq!(
        animation["channels"],
        json!([
            {"sampler": 3, "target": {"node": 1, "path": "weights"}},
            {"sampler": 1, "target": {"node": 3, "path": "rotation"}},
            {"sampler": 2, "target": {"node": 0, "path": "weights"}},
            {"sampler": 0, "target": {"node": 2, "path": "translation"}},
            {"sampler": 0, "target": {"node": 2, "path": "scale"}}
        ])
    );
    let id = |value: &serde_json::Value| usize::try_from(value.as_u64().unwrap()).unwrap();
    let value = |index, shape, count| {
        let accessor = &doc["accessors"][index];
        assert_eq!(accessor["type"], shape);
        assert_eq!(accessor["componentType"], 5126);
        assert_eq!(accessor["count"], count);
        let view = &doc["bufferViews"][id(&accessor["bufferView"])];
        assert_eq!(id(&view["byteOffset"]) % 4, 0);
        assert!(view.get("target").is_none());
        support::f32s(&bytes, &doc, index)
    };
    let samplers = animation["samplers"].as_array().unwrap();
    let time_id = id(&samplers[0]["input"]);
    let end = 1.0f32.next_up();
    assert_eq!(value(time_id, "SCALAR", 2), [0.25, end]);
    assert_eq!(doc["accessors"][time_id]["min"], json!([0.25]));
    assert_eq!(doc["accessors"][time_id]["max"], json!([f64::from(end)]));

    let s = std::f32::consts::FRAC_1_SQRT_2;
    for (index, (mode, shape, count, expected)) in [
        ("LINEAR", "VEC3", 2, &[-2., 0., 4., 6., 4., 0.][..]),
        ("LINEAR", "VEC4", 2, &[0., 0., 0., 1., 0., 0., s, s]),
        ("LINEAR", "SCALAR", 4, &[0.25, 0.5, 0.75, -0.25]),
        ("STEP", "SCALAR", 4, &[0.75, 0.25, 0.5, 1.25]),
    ]
    .into_iter()
    .enumerate()
    {
        let sampler = &samplers[index];
        assert_eq!(sampler["interpolation"], mode);
        assert_eq!(id(&sampler["input"]), time_id);
        let output = id(&sampler["output"]);
        assert_eq!(output, time_id + index + 1);
        assert_eq!(value(output, shape, count), expected);
    }
    let single = &doc["animations"][1]["samplers"][0];
    let single_input = id(&single["input"]);
    assert_eq!(value(single_input, "SCALAR", 1), [0.]);
    assert_eq!(doc["accessors"][single_input]["min"], json!([0]));
    assert_eq!(doc["accessors"][single_input]["max"], json!([0]));
    assert_eq!(value(id(&single["output"]), "VEC3", 1), [1., 2., 3.]);

    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json = std::str::from_utf8(&bytes[20..20 + json_len]).unwrap();
    assert!(json.contains(
        r#""animations":[{"channels":[{"sampler":3,"target":{"node":1,"path":"weights"}"#
    ));
    assert!(json.contains(&format!(
        r#""samplers":[{{"input":{time_id},"interpolation":"LINEAR","output":{}}}"#,
        time_id + 1
    )));
}

#[test]
fn animation_bytes_are_stable_between_writes_and_threads() {
    let scene = support::animated(&Meter::unlimited()).unwrap();
    let expected = support::write(&scene, Options::default()).0;
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..2 {
                    assert_eq!(support::write(&scene, Options::default()).0, expected);
                }
            });
        }
    });
}

#[test]
fn writes_animated_glb_for_parity() {
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-animation-parity");
    let path = std::env::var_os("UNITYRS_GLTF_PARITY_GLB")
        .map_or_else(|| scratch.path().join("animated.glb"), Into::into);
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    let half_angle = 179.9999_f64.to_radians() / 2.;
    key(&mut scene, Kind::AnimationRotation, 6, half_angle.sin());
    key(&mut scene, Kind::AnimationRotation, 7, half_angle.cos());
    let mut file = std::fs::File::create(path).unwrap();
    write_glb(&scene, &Options::default(), &Meter::unlimited(), &mut file).unwrap();
}

#[test]
fn metadata_only_animation_emits_no_glb_tables() {
    let meter = Meter::unlimited();
    let mut scene = Scene::new(&meter);
    let mut clip = Animation::new(&meter);
    clip.extras
        .push((
            text(&meter, "source").unwrap(),
            text(&meter, "metadata").unwrap(),
        ))
        .unwrap();
    scene.animations.push(clip).unwrap();
    assert_eq!(
        support::write(&scene, Options::default()).0,
        include_bytes!("goldens/empty.glb")
    );
    assert_eq!(scene.animations.as_slice()[0].extras.len(), 1);
}

fn key(scene: &mut Scene, kind: Kind, scalar: usize, value: f64) {
    let accessor = scene
        .accessors
        .as_mut_slice()
        .iter_mut()
        .find(|a| a.kind == kind)
        .unwrap();
    let Data::Float(data) = &mut accessor.data else {
        panic!("fixture data")
    };
    data.as_mut_slice()[scalar] = value;
}

fn active(scene: &mut Scene) -> &mut Animation {
    &mut scene.animations.as_mut_slice()[1]
}

#[test]
fn malformed_animations_refuse_before_output_in_both_modes() {
    const TIME_ERROR: &str =
        "animation times must be finite, nonnegative and strictly increasing in f32";
    type Case = (fn(&mut Scene), &'static str);
    let cases: [Case; 15] = [
        (
            |s| active(s).channels.as_mut_slice()[0].node = 100,
            "animation target node index",
        ),
        (
            |s| active(s).channels.as_mut_slice()[0].sampler = 100,
            "animation sampler index",
        ),
        (
            |s| active(s).samplers.as_mut_slice()[1].input = 100,
            "accessor index",
        ),
        (
            |s| active(s).samplers.as_mut_slice()[1].output = 100,
            "accessor index",
        ),
        (
            |s| active(s).samplers.as_mut_slice()[1].output = 0,
            "accessor semantic",
        ),
        (
            |s| active(s).channels.as_mut_slice()[0].node = 0,
            "duplicate animation target",
        ),
        (
            |s| active(s).channels.as_mut_slice()[0].node = 3,
            "animation weights need a mesh",
        ),
        (
            |s| active(s).channels.as_mut_slice()[3].node = 1,
            "animation TRS targets a skinned mesh attachment",
        ),
        (
            |s| {
                let single_time = s.animations.as_slice()[2].samplers.as_slice()[0].input;
                active(s).samplers.as_mut_slice()[4].input = single_time;
            },
            "animation output count",
        ),
        (
            |s| key(s, Kind::AnimationTime, 1, 0.250_000_001),
            TIME_ERROR,
        ),
        (|s| key(s, Kind::AnimationTime, 0, -1.), TIME_ERROR),
        (|s| key(s, Kind::AnimationTime, 1, f64::NAN), TIME_ERROR),
        (
            |s| key(s, Kind::AnimationVec3, 2, f64::MAX),
            "non-finite f32 animation output",
        ),
        (
            |s| key(s, Kind::AnimationRotation, 3, 0.),
            "animation rotation must be unit length",
        ),
        (
            |s| key(s, Kind::AnimationRotation, 3, -1.),
            ROTATION_SIGN_ERROR,
        ),
    ];
    for (mutate, message) in cases {
        let mut scene = support::animated(&Meter::unlimited()).unwrap();
        mutate(&mut scene);
        for normalise in [true, false] {
            refuses(&scene, normalise, message);
        }
    }
}

fn refuses(scene: &Scene, normalise: bool, message: &str) {
    let meter = Meter::unlimited();
    let mut out = Vec::new();
    let result = write_glb(scene, &Options { normalise }, &meter, &mut out);
    assert!(
        matches!(&result, Err(GltfError::Invalid(actual)) if *actual == message),
        "expected {message:?}, got {result:?} (normalise={normalise})"
    );
    assert!(out.is_empty(), "{message}");
    assert_eq!(meter.used(), 0, "{message}");
}

#[test]
fn weights_cannot_target_a_mesh_omitted_by_normalisation() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    key(&mut scene, Kind::Position, 0, f64::NAN);
    refuses(&scene, true, "animation weights target an omitted mesh");
}

#[test]
fn translation_targets_a_node_after_its_skinned_mesh_is_omitted() {
    let meter = Meter::unlimited();
    let mut scene = support::animated(&meter).unwrap();
    key(&mut scene, Kind::Position, 0, f64::NAN);
    let channels = &mut active(&mut scene).channels;
    channels.as_mut_slice()[0] = AnimationChannel {
        sampler: 1,
        node: 1,
        target: AnimationTarget::Translation,
    };
    channels.truncate(1);
    scene.nodes.as_mut_slice()[1].children.push(4).unwrap();
    scene.nodes.push(Node::new(&meter)).unwrap();

    let (bytes, _) = support::write(&scene, Options::default());
    let doc = support::json(&bytes);
    assert_eq!(doc["nodes"].as_array().unwrap().len(), 5);
    assert!(doc["nodes"][1].get("mesh").is_none());
    assert!(doc["nodes"][1].get("skin").is_none());
    assert_eq!(doc["nodes"][1]["children"], json!([4]));
    assert_eq!(
        doc["animations"][0]["channels"],
        json!([{"sampler": 0, "target": {"node": 1, "path": "translation"}}])
    );
}

#[test]
fn linear_rotation_refuses_a_negative_dot_product_after_f32_rounding() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    // Both quaternions have unit length to f64 precision. The first three
    // products cancel before rounding, leaving a small positive dot product.
    let root = 26_f64.sqrt();
    let keys = [
        0.5,
        0.5,
        0.5,
        0.5,
        3. / root,
        1. / root,
        -4. / root,
        1e-8 / root,
    ];
    let dot = |keys: [f64; 8]| {
        keys[..4]
            .iter()
            .zip(&keys[4..])
            .map(|(a, b)| a * b)
            .sum::<f64>()
    };
    assert!(dot(keys) > 0.);
    #[allow(clippy::cast_possible_truncation)]
    let rounded = keys.map(|value| f64::from(value as f32));
    assert!(dot(rounded) < 0.);
    for (i, value) in keys.into_iter().enumerate() {
        key(&mut scene, Kind::AnimationRotation, i, value);
    }
    for normalise in [true, false] {
        refuses(&scene, normalise, ROTATION_SIGN_ERROR);
    }
}

#[test]
fn linear_rotation_refuses_a_negative_dot_product_hidden_by_cancellation() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    let x = 0.707_106_77_f32;
    let t = 2_f32.powi(-30);
    for (i, value) in [x, t, x, 0., x, -t, -x, 0.].into_iter().enumerate() {
        key(&mut scene, Kind::AnimationRotation, i, f64::from(value));
    }
    for normalise in [true, false] {
        refuses(&scene, normalise, ROTATION_SIGN_ERROR);
    }
}

#[test]
fn step_rotation_preserves_opposite_signs_in_a_shared_accessor() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    let keys = [0_f32, 0., 0., 1., 0., 0., 0., -1.];
    for (i, value) in keys.into_iter().enumerate() {
        key(&mut scene, Kind::AnimationRotation, i, f64::from(value));
    }
    active(&mut scene).samplers.as_mut_slice()[2].interpolation = Interpolation::Step;
    for normalise in [true, false] {
        let (bytes, _) = support::write(&scene, Options { normalise });
        let doc = support::json(&bytes);
        let sampler = &doc["animations"][0]["samplers"][1];
        assert_eq!(sampler["interpolation"], "STEP");
        let output = usize::try_from(sampler["output"].as_u64().unwrap()).unwrap();
        assert_eq!(support::f32s(&bytes, &doc, output), keys);
    }

    let clip = active(&mut scene);
    let sampler = clip.samplers.len();
    let mut linear = clip.samplers.as_slice()[2];
    linear.interpolation = Interpolation::Linear;
    clip.samplers.push(linear).unwrap();
    clip.channels
        .push(AnimationChannel {
            sampler,
            node: 2,
            target: AnimationTarget::Rotation,
        })
        .unwrap();
    for normalise in [true, false] {
        refuses(&scene, normalise, ROTATION_SIGN_ERROR);
    }
}

#[test]
fn negative_zero_is_a_valid_first_animation_time() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    key(&mut scene, Kind::AnimationTime, 0, -0.);
    let (bytes, _) = support::write(&scene, Options::default());
    let doc = support::json(&bytes);
    let input = doc["animations"][0]["samplers"][0]["input"]
        .as_u64()
        .unwrap();
    let index = usize::try_from(input).unwrap();
    assert_eq!(
        support::f32s(&bytes, &doc, index)[0].to_bits(),
        (-0.0f32).to_bits()
    );
    assert_eq!(doc["accessors"][index]["min"], json!([0]));
}
