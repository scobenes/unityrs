#![cfg(feature = "gltf")]
#![allow(clippy::float_cmp)]
#[allow(
    dead_code,
    reason = "replay also uses the specialized fixture variants"
)]
mod anim_support;
#[path = "anim_collection/review.rs"]
mod review;

use serde_json::Value as J;
use std::{path::Path, process::Command, sync::Arc};
use unityrs_core::{
    Bytes, RawStr,
    env::{EnvOptions, Environment, Source},
    files::File,
    limits::Meter,
};
use unityrs_export::gltf::{Collected, ExportError, collect};
use unityrs_gltf::{Profile, Scene, UnlitExtras, write_glb};
use unityrs_scene::{Graph, GraphOptions, Unit};
use unityrs_testkit::{
    child,
    ngui::{Object, V, st, vector},
    scene3d::{
        self as s,
        anim::{
            self,
            playback::{self as p, Key},
        },
        mesh as m,
    },
    scratch::Scratch,
};

fn input(bytes: Vec<u8>, prefab: u32) -> (Environment, File, Graph, Arc<Meter>) {
    let env = Environment::new(EnvOptions::unitypy());
    let root = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(bytes),
        })
        .unwrap();
    let file = match &root {
        File::Bundle(_) => root.container_entry(prefab as usize).unwrap(),
        _ => root.clone(),
    };
    let File::Serialized(file) = file else {
        panic!("serialized fixture")
    };
    let unit = Unit::Prefab(file.object(1).unwrap());
    let meter = unit.meter().clone();
    let graph = unit.graph(&env, &GraphOptions::default(), &meter).unwrap();
    (env, root, graph, meter)
}

#[allow(clippy::result_large_err)]
fn animated(
    env: &Environment,
    root: &File,
    graph: &Graph,
    meter: &Arc<Meter>,
    profile: &[u8],
) -> Result<Collected, ExportError> {
    let profile = Profile::parse(profile, meter)?;
    collect(
        env,
        std::slice::from_ref(root),
        graph,
        &profile,
        &UnlitExtras,
        meter,
    )
}

const ALL: &[u8] = br#"{"profile_version":1,"clips":"all_discovered","sample_rate":4}"#;

fn glb(scene: &Scene, meter: &Arc<Meter>) -> Vec<u8> {
    let mut out = Vec::new();
    write_glb(scene, &unityrs_gltf::Options::default(), meter, &mut out).unwrap();
    out
}

fn doc(bytes: &[u8]) -> J {
    let size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + size]).unwrap()
}

fn accessor(bytes: &[u8], doc: &J, index: &J) -> Vec<f64> {
    let uint = |row: &J| usize::try_from(row.as_u64().unwrap()).unwrap();
    let a = &doc["accessors"][uint(index)];
    let view = &doc["bufferViews"][uint(&a["bufferView"])];
    let bin_at = 28 + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let offset = bin_at + uint(&view["byteOffset"]);
    let width = match a["type"].as_str().unwrap() {
        "SCALAR" => 1,
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT4" => 16,
        _ => panic!("type"),
    };
    let stride = if a["componentType"] == 5126 { 4 } else { 2 };
    bytes[offset..offset + stride * width * uint(&a["count"])]
        .chunks_exact(stride)
        .map(|word| {
            if stride == 4 {
                f64::from(f32::from_le_bytes(word.try_into().unwrap()))
            } else {
                f64::from(u16::from_le_bytes(word.try_into().unwrap()))
            }
        })
        .collect()
}

fn validate(path: &Path) {
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        return;
    };
    let out = child::output(Command::new(exe).args(["--stdout", "--all"]).arg(path)).unwrap();
    let report: J = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{report}");
    assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
    assert_eq!(report["issues"]["numErrors"], 0, "{report}");
    assert_eq!(report["issues"]["numWarnings"], 0, "{report}");
}

#[test]
fn standalone_generic_and_legacy_write_distinct_instance_weights() {
    let scratch = Scratch::new("animated-collect");
    for legacy in [false, true] {
        let (env, root, graph, meter) = input(anim_support::bundle(legacy), 1);
        let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
        assert_eq!(collected.clips.len(), 1);
        let bytes = glb(&collected.scene, &meter);
        let scene = doc(&bytes);
        assert_eq!(scene["nodes"][0]["mesh"], scene["nodes"][1]["mesh"]);
        assert_eq!(scene["nodes"][0]["weights"][0], 0.125);
        assert_eq!(scene["nodes"][1]["weights"][0], 0.375);
        let anim = &scene["animations"][0];
        let channels = anim["channels"].as_array().unwrap();
        let weight = channels
            .iter()
            .find(|c| c["target"]["path"] == "weights")
            .unwrap();
        assert_eq!(weight["target"]["node"], 0);
        let sampler =
            &anim["samplers"][usize::try_from(weight["sampler"].as_u64().unwrap()).unwrap()];
        assert_eq!(sampler["interpolation"], "STEP");
        assert_eq!(
            accessor(&bytes, &scene, &sampler["input"]),
            [0., 0.25, 0.5, 0.75, 1.]
        );
        let last = f64::from(17.5_f32 / 80.);
        assert_eq!(
            accessor(&bytes, &scene, &sampler["output"]),
            [0.15625, last, last, last, last]
        );
        assert!(
            collected
                .scene
                .collection_issues
                .as_slice()
                .iter()
                .any(|i| i.reason == "animation_morph_last_frame_approximation")
        );
        let path = if legacy {
            std::env::var_os("UNITYRS_GLTF_COLLECTED_GLB")
                .map_or_else(|| scratch.join("legacy.glb"), Into::into)
        } else {
            scratch.join("generic.glb")
        };
        std::fs::write(&path, &bytes).unwrap();
        validate(&path);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    let again = animated(&env, &root, &graph, &meter, ALL).unwrap();
                    assert_eq!(glb(&again.scene, &meter), bytes);
                });
            }
        });
    }
}

#[test]
fn selected_animation_refuses_a_filtered_target_and_a_bad_profile() {
    let (env, root, graph, meter) = input(anim_support::bundle(false), 1);
    let e = animated(
        &env,
        &root,
        &graph,
        &meter,
        br#"{"profile_version":1,"clips":"all_discovered","exclusions":["root"]}"#,
    )
    .unwrap_err();
    assert!(matches!(
        e,
        ExportError::Animation(unityrs_anim::AnimError::FilteredTarget(0))
    ));
    for (profile, cause) in [
        (
            br#"{"profile_version":1,"clip_states":[{}]}"#.as_slice(),
            "unsupported_controller_evaluation",
        ),
        (
            br#"{"profile_version":1,"skin_influences":8,"normalise":false}"#.as_slice(),
            "unsupported_skin_influences",
        ),
        (
            br#"{"profile_version":1,"sample_rate":1025}"#.as_slice(),
            "profile sample rate",
        ),
    ] {
        assert!(
            animated(&env, &root, &graph, &meter, profile)
                .unwrap_err()
                .to_string()
                .contains(cause)
        );
    }
}

fn file(objects: &[Object]) -> Vec<u8> {
    s::file(s::Layout::Unity2022, &[], objects).unwrap()
}

#[test]
fn detached_batches_refuse_animation_of_an_ancestor_in_both_modes() {
    let (env, root, graph, meter) = input(
        file(&[
            Object::new(1, 1, s::game_object(b"root", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[(0, 5)], false)),
            anim::legacy(3, (0, 9), &[]),
            Object::new(4, 1, s::game_object(b"batch", &[(0, 5), (0, 6), (0, 8)])),
            Object::new(5, 4, s::transform((0, 4), (0, 2), &[], false)),
            Object::new(6, 33, s::mesh_filter((0, 4), (0, 7))),
            m::mesh(
                7,
                b"mesh",
                &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
                &[0, 1, 2],
                m::MeshVersion::V2022,
            ),
            Object::new(8, 23, s::mesh_renderer((0, 4), &[], (0, 1))),
            p::explicit(
                9,
                1.,
                &[(
                    "m_PositionCurves",
                    vec![p::curve(
                        "",
                        &[Key::new(0., [1., 0., 0.])],
                        [2, 2],
                        None,
                        true,
                        None,
                    )],
                )],
            ),
        ]),
        0,
    );
    for mode in ["unbatch", "keep_combined"] {
        let profile =
            serde_json::json!({"profile_version":1,"clips":"all_discovered","static_batch":mode});
        assert!(matches!(
            animated(
                &env,
                &root,
                &graph,
                &meter,
                &serde_json::to_vec(&profile).unwrap()
            ),
            Err(ExportError::AnimatedStaticBatch)
        ));
    }
}

fn row(translation: [f32; 3]) -> [f32; 16] {
    let [x, y, z] = translation;
    [1., 0., 0., x, 0., 1., 0., y, 0., 0., 1., z, 0., 0., 0., 1.]
}

fn skin_mesh(positions: [[f32; 3]; 3], inverses: [[f32; 3]; 4]) -> Object {
    let mut mesh = m::mesh(7, b"skin", &positions, &[0, 1, 2], m::MeshVersion::V2022);
    let shape = |offset, format, dimension| {
        st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(offset)),
                ("format", V::UInt8(format)),
                ("dimension", V::UInt8(dimension)),
            ],
        )
    };
    let mut channels = vec![shape(0, 0, 0); 14];
    channels[0] = shape(0, 0, 3);
    channels[12] = shape(12, 0, 4);
    channels[13] = shape(28, 10, 4);
    let mut bytes = Vec::new();
    for position in positions {
        for x in position.into_iter().chain([0.125_f32, 0.25, 0.375, 0.25]) {
            bytes.extend(x.to_le_bytes());
        }
        for joint in 0..4_u32 {
            bytes.extend(joint.to_le_bytes());
        }
    }
    bytes.resize(bytes.len().next_multiple_of(16), 0);
    s::set(
        &mut mesh.value,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(3)),
                ("m_Channels", vector(channels[0].clone(), channels)),
                ("m_DataSize", V::Bytes(bytes)),
            ],
        ),
    );
    s::set(
        &mut mesh.value,
        "m_BindPose",
        vector(
            m::matrix([0.; 16]),
            inverses.map(|p| m::matrix(row(p))).to_vec(),
        ),
    );
    mesh
}

#[test]
#[allow(clippy::too_many_lines)]
fn deformation_uses_all_four_influences_on_a_separate_skin_attachment() {
    let positions = [[1., 2., 3.], [0., 1., 0.], [0., 0., 0.]];
    let inverses = [[-1., 0., 0.], [0., -2., 0.], [0., 0., -3.], [-1., -1., -1.]];
    let joint_names = ["joint a", "joint b", "joint c", "joint d"];
    let translations = [[3., 0., 0.], [0., 4., 0.], [0., 0., 5.], [6., 7., 8.]];
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let mut objects = vec![
        Object::new(1, 1, s::game_object(b"root", &[(0, 2), (0, 3), (0, 6)])),
        Object::new(
            2,
            4,
            s::transform((0, 1), (0, 0), &[(0, 21), (0, 23), (0, 25), (0, 27)], false),
        ),
        anim::legacy(3, (0, 9), &[]),
        Object::new(
            6,
            137,
            s::skinned_mesh_renderer((0, 1), (0, 7), &[(0, 21), (0, 23), (0, 25), (0, 27)], &[]),
        ),
        skin_mesh(positions, inverses),
        p::explicit(
            9,
            4.,
            &[
                (
                    "m_PositionCurves",
                    joint_names
                        .iter()
                        .zip(translations)
                        .map(|(name, end)| {
                            p::curve(
                                name,
                                &[Key::new(0., [0.; 3]), Key::new(1., end)],
                                [2, 2],
                                None,
                                true,
                                None,
                            )
                        })
                        .collect(),
                ),
                (
                    "m_RotationCurves",
                    vec![p::curve(
                        "joint a",
                        &[Key::new(0., [0., 0., 0., 1.]), Key::new(1., [0., 0., q, q])],
                        [2, 2],
                        None,
                        true,
                        None,
                    )],
                ),
            ],
        ),
    ];
    for (name, id) in joint_names.into_iter().zip([20, 22, 24, 26]) {
        objects.push(Object::new(
            id,
            1,
            s::game_object(name.as_bytes(), &[(0, id + 1)]),
        ));
        objects.push(Object::new(
            id + 1,
            4,
            s::transform((0, id), (0, 2), &[], false),
        ));
    }
    let (env, root, graph, meter) = input(file(&objects), 0);
    let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
    let bytes = glb(&collected.scene, &meter);
    let doc = doc(&bytes);
    let scratch = Scratch::new("animated-skin");
    let path = scratch.join("skin.glb");
    std::fs::write(&path, &bytes).unwrap();
    validate(&path);
    let skin = &doc["skins"][0];
    let attachment = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["skin"] == 0)
        .unwrap();
    assert_ne!(attachment, 0);
    let primitive = &doc["meshes"][0]["primitives"][0]["attributes"];
    let position = accessor(&bytes, &doc, &primitive["POSITION"]);
    let weights = accessor(&bytes, &doc, &primitive["WEIGHTS_0"]);
    let joints = accessor(&bytes, &doc, &primitive["JOINTS_0"]);
    let matrices = accessor(&bytes, &doc, &skin["inverseBindMatrices"]);
    let animation = &doc["animations"][0];
    assert_eq!(skin["joints"], serde_json::json!([1, 2, 3, 4]));
    let reconstruct = |node: usize, property: &str, width: usize, time: f64| {
        let channel = animation["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["target"]["node"] == node && c["target"]["path"] == property)
            .unwrap();
        let sampler = usize::try_from(channel["sampler"].as_u64().unwrap()).unwrap();
        let sampler = &animation["samplers"][sampler];
        let times = accessor(&bytes, &doc, &sampler["input"]);
        let values = accessor(&bytes, &doc, &sampler["output"]);
        let right = times.partition_point(|&t| t <= time);
        if right == times.len() {
            return values[values.len() - width..].to_vec();
        }
        let left = right - 1;
        let u = (time - times[left]) / (times[right] - times[left]);
        let a = &values[left * width..right * width];
        let b = &values[right * width..(right + 1) * width];
        if property == "rotation" {
            // The one-axis fixture makes SLERP interpolation of half angles.
            let angle_a = a[2].atan2(a[3]);
            let angle_b = b[2].atan2(b[3]);
            let (sine, cosine) = (angle_a + u * (angle_b - angle_a)).sin_cos();
            vec![0., 0., sine, cosine]
        } else {
            a.iter().zip(b).map(|(a, b)| a + (b - a) * u).collect()
        }
    };

    let rotate_z =
        |p: [f64; 3], sin: f64, cos: f64| [cos * p[0] - sin * p[1], sin * p[0] + cos * p[1], p[2]];
    for (time, tolerance) in [(0.375_976_562_5, 2.5e-4), (1., 2e-6)] {
        let u = time * time * (3. - 2. * time);
        let mut unity = [0.; 3];
        let mut emitted = [0.; 3];
        for (i, weight) in [0.125_f64, 0.25, 0.375, 0.25].into_iter().enumerate() {
            assert_eq!(joints[i], f64::from(u32::try_from(i).unwrap()));
            assert_eq!(weights[i], weight);
            let matrix = &matrices[i * 16..(i + 1) * 16];
            let mut source = [0.; 3];
            let mut gltf = [0.; 3];
            for axis in 0..3 {
                source[axis] = f64::from(positions[0][axis] + inverses[i][axis]);
                gltf[axis] = matrix[12 + axis]
                    + (0..3)
                        .map(|column| matrix[column * 4 + axis] * position[column])
                        .sum::<f64>();
            }
            if i == 0 {
                let half_angle = (f64::from(q) * u).atan2(1. + (f64::from(q) - 1.) * u);
                let (sine, cosine) = (2. * half_angle).sin_cos();
                source = rotate_z(source, sine, cosine);
                let rotation = reconstruct(1, "rotation", 4, time);
                let (z, w) = (rotation[2], rotation[3]);
                let norm = z * z + w * w;
                gltf = rotate_z(gltf, 2. * z * w / norm, (w * w - z * z) / norm);
            }
            let translation = reconstruct(i + 1, "translation", 3, time);
            for axis in 0..3 {
                unity[axis] += weight * (source[axis] + f64::from(translations[i][axis]) * u);
                emitted[axis] += weight * (gltf[axis] + translation[axis]);
            }
        }
        unity[0] = -unity[0];
        for (a, b) in emitted.into_iter().zip(unity) {
            assert!((a - b).abs() < tolerance, "{time}: {a} vs {b}");
        }
    }
}

fn clip_input(clip: Object) -> (Environment, File, Graph, Arc<Meter>) {
    input(
        file(&[
            Object::new(1, 1, s::game_object(b"root", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            anim::legacy(3, (0, 9), &[]),
            clip,
        ]),
        0,
    )
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn mixed_jumps_use_exact_sides_when_local_time_rounds_up_or_down() {
    let start = 0.1_f32;
    for (jump, rounds_up) in [(0.35_f32, true), (1.1_f32, false)] {
        let (before, after) = if rounds_up { (0., 1e-13_f32) } else { (2., 7.) };
        let mut a = Key::new(start, [0., 0., before]);
        a.outgoing = [0., 0., f32::INFINITY];
        let mut b = Key::new(jump, [3., 0., after]);
        b.outgoing = [0., 0., f32::INFINITY];
        let c = Key::new(2., [4., 0., after]);
        let clip = p::explicit(
            9,
            4.,
            &[(
                "m_PositionCurves",
                vec![p::curve("", &[a, b, c], [2, 2], None, true, None)],
            )],
        );
        let (env, root, graph, meter) = clip_input(clip);
        let collected = animated(
            &env,
            &root,
            &graph,
            &meter,
            br#"{"profile_version":1,"clips":"all_discovered","sample_rate":5.5}"#,
        )
        .unwrap();
        let bytes = glb(&collected.scene, &meter);
        let doc = doc(&bytes);
        let sampler = &doc["animations"][0]["samplers"][0];
        assert_eq!(sampler["interpolation"], "LINEAR");
        let times = accessor(&bytes, &doc, &sampler["input"]);
        let output = accessor(&bytes, &doc, &sampler["output"]);
        let exact = f64::from(jump) - f64::from(start);
        let rounded = exact as f32;
        assert_eq!(f64::from(rounded) > exact, rounds_up);
        let i = times.iter().position(|&t| t == f64::from(rounded)).unwrap();
        assert_eq!(times[i - 1], f64::from(rounded.next_down()));
        assert_eq!(
            output[(i - 1) * 3..(i + 1) * 3],
            [-3., 0., f64::from(before), -3., 0., f64::from(after)]
        );
    }
}

#[test]
fn animation_refuses_conflicting_timestamps_and_excessive_schedule_work() {
    let mut a = Key::new(-1000., [0.]);
    a.outgoing = [f32::INFINITY];
    let mut b = Key::new(0., [1.]);
    b.outgoing = [f32::INFINITY];
    let c = Key::new(f32::from_bits(1), [2.]);
    let (env, root, graph, meter) = clip_input(p::explicit(
        9,
        1.,
        &[(
            "m_FloatCurves",
            vec![p::curve(
                "",
                &[a, b, c],
                [2, 2],
                None,
                true,
                Some(("m_LocalPosition.x", 4)),
            )],
        )],
    ));
    let result = animated(&env, &root, &graph, &meter, ALL).unwrap_err();
    assert!(matches!(result, ExportError::Bake("rounded jump bracket")));
    let result = animated(
        &env,
        &root,
        &graph,
        &meter,
        br#"{"profile_version":1,"clips":"all_discovered","sample_rate":1024}"#,
    )
    .unwrap_err();
    assert!(matches!(result, ExportError::Bake("schedule key cap")));
}

#[test]
fn metadata_only_clips_retain_serialized_values_and_empty_clips_refuse() {
    let metadata = p::explicit(
        9,
        1.,
        &[(
            "m_FloatCurves",
            vec![p::curve(
                "",
                &[Key::new(3., [0.5])],
                [2, 2],
                None,
                true,
                Some(("material.opacity", 23)),
            )],
        )],
    );
    let (env, root, graph, meter) = clip_input(metadata);
    let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
    assert_eq!(collected.clips.len(), 1);
    assert!(collected.clips.as_slice()[0].has_metadata_tracks());
    assert!(
        doc(&glb(&collected.scene, &meter))
            .get("animations")
            .is_none()
    );
    let (env, root, graph, meter) = clip_input(p::explicit(9, 1., &[]));
    assert!(matches!(
        animated(&env, &root, &graph, &meter, ALL),
        Err(ExportError::Animation(
            unityrs_anim::AnimError::NoPlayableChannels
        ))
    ));
}

#[test]
fn full_euler_turn_is_subdivided_and_mirrored() {
    let mut start = Key::new(0., [0.; 3]);
    start.outgoing = [0., 360., 0.];
    let mut end = Key::new(1., [0., 360., 0.]);
    end.incoming = [0., 360., 0.];
    let clip = p::explicit(
        9,
        1.,
        &[(
            "m_EulerCurves",
            vec![p::curve("", &[start, end], [2, 2], Some(4), true, None)],
        )],
    );
    let (env, root, graph, meter) = clip_input(clip);
    let collected = animated(
        &env,
        &root,
        &graph,
        &meter,
        br#"{"profile_version":1,"clips":"all_discovered"}"#,
    )
    .unwrap();
    let bytes = glb(&collected.scene, &meter);
    let doc = doc(&bytes);
    let sampler = &doc["animations"][0]["samplers"][0];
    let times = accessor(&bytes, &doc, &sampler["input"]);
    let values = accessor(&bytes, &doc, &sampler["output"]);
    let quarter = times.iter().position(|&key| key == 0.25).unwrap();
    let expected = f64::from(std::f32::consts::FRAC_1_SQRT_2);
    assert_eq!(
        values[quarter * 4..quarter * 4 + 4],
        [0., -expected, 0., expected]
    );
    assert_eq!(
        doc["animations"][0]["channels"][0]["target"]["path"],
        "rotation"
    );
    let scratch = Scratch::new("animated-turn");
    let path = std::env::var_os("UNITYRS_GLTF_TURN_GLB")
        .map_or_else(|| scratch.join("turn.glb"), Into::into);
    std::fs::write(&path, bytes).unwrap();
    validate(&path);
}

#[test]
fn large_translation_uses_f32_ulp_rounding_at_an_off_key_endpoint() {
    let mut clip = p::explicit(9, 1., &[]);
    let mut words = Vec::new();
    p::frame(
        &mut words,
        0.,
        &[
            (0, [0_f32, 0., 0.1, 1_000_000.].map(f32::to_bits)),
            (1, [0; 4]),
            (2, [0; 4]),
        ],
    );
    p::frame(&mut words, 2., &[(0, [0; 4]), (1, [0; 4]), (2, [0; 4])]);
    p::add_compiled(
        &mut clip,
        s::Layout::Unity2022,
        &[anim::Binding {
            class: 4,
            attribute: 1,
            ..anim::Binding::default()
        }],
        &[],
        p::Storage {
            interval: [0., 1.],
            streamed_count: 3,
            words: &words,
            ..p::Storage::default()
        },
    );
    let (env, root, graph, meter) = clip_input(clip);
    let collected = animated(&env, &root, &graph, &meter, ALL).unwrap();
    let bytes = glb(&collected.scene, &meter);
    let doc = doc(&bytes);
    let sampler = &doc["animations"][0]["samplers"][0];
    let values = accessor(&bytes, &doc, &sampler["output"]);
    let source = -1_000_000. - f64::from(0.1_f32);
    assert_ne!(values[values.len() - 3], source);
    assert!((values[values.len() - 3] - source).abs() <= 0.03125);
    let scratch = Scratch::new("animated-large");
    let path = std::env::var_os("UNITYRS_GLTF_LARGE_GLB")
        .map_or_else(|| scratch.join("large.glb"), Into::into);
    std::fs::write(&path, bytes).unwrap();
    validate(&path);
}

#[test]
fn budget_refusal_remains_typed_and_releases_collection_storage() {
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::{error::LimitKind, limits::ChargeError};
    let (env, root, graph, _) = input(anim_support::bundle(false), 1);
    let profile = Profile::parse(ALL, &Meter::unlimited()).unwrap();
    let budget = Meter::with_decode_work(u64::MAX, 64, Arc::new(Unbounded), Reservation::Input);
    let result = collect(
        &env,
        std::slice::from_ref(&root),
        &graph,
        &profile,
        &UnlitExtras,
        &budget,
    );
    assert!(matches!(
        result,
        // Static collection runs before the animation callback.
        Err(ExportError::Gltf(unityrs_gltf::GltfError::Charge(ChargeError::Limit(limit))))
            if limit.kind() == LimitKind::DecodeWork && limit.max() == 64 && limit.value() > 64
    ));
    assert_eq!(budget.used(), 0);
}
