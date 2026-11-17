#![allow(clippy::float_cmp)]
use serde_json::json;
use unityrs_core::limits::Meter;
use unityrs_gltf::*;

mod support;

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
    let cases: [fn(&mut Scene); 15] = [
        |s| active(s).channels.as_mut_slice()[0].node = 100,
        |s| active(s).channels.as_mut_slice()[0].sampler = 100,
        |s| active(s).samplers.as_mut_slice()[1].input = 100,
        |s| active(s).samplers.as_mut_slice()[1].output = 100,
        |s| active(s).samplers.as_mut_slice()[1].output = 0,
        |s| active(s).channels.as_mut_slice()[0].node = 0,
        |s| active(s).channels.as_mut_slice()[0].node = 3,
        |s| active(s).channels.as_mut_slice()[3].node = 1,
        |s| {
            let single_time = s.animations.as_slice()[2].samplers.as_slice()[0].input;
            active(s).samplers.as_mut_slice()[4].input = single_time;
        },
        |s| key(s, Kind::AnimationTime, 1, 0.250_000_001),
        |s| key(s, Kind::AnimationTime, 0, -1.),
        |s| key(s, Kind::AnimationTime, 1, f64::NAN),
        |s| key(s, Kind::AnimationVec3, 2, f64::MAX),
        |s| key(s, Kind::AnimationRotation, 3, 0.),
        |s| key(s, Kind::AnimationRotation, 3, -1.),
    ];
    for (i, mutate) in cases.into_iter().enumerate() {
        let mut scene = support::animated(&Meter::unlimited()).unwrap();
        mutate(&mut scene);
        for normalise in [true, false] {
            refuses(&scene, normalise, i);
        }
    }
}

fn refuses(scene: &Scene, normalise: bool, case: usize) {
    let meter = Meter::unlimited();
    let mut out = Vec::new();
    let result = write_glb(scene, &Options { normalise }, &meter, &mut out);
    assert!(
        matches!(&result, Err(GltfError::Invalid(_))),
        "case {case}: {result:?}"
    );
    assert!(out.is_empty(), "case {case}");
    assert_eq!(meter.used(), 0, "case {case}");
}

#[test]
fn weights_cannot_target_a_mesh_omitted_by_normalisation() {
    let mut scene = support::animated(&Meter::unlimited()).unwrap();
    key(&mut scene, Kind::Position, 0, f64::NAN);
    refuses(&scene, true, 0);
}
