//! Synthetic bundle, Editor-shaped observations and predeclared ranges.
use super::rules::move_interior;
use crate::anim_support::{CAB, CLIP, PREFAB, bundle};
use crate::{J, Path, compare, derive, digest};
use serde_json::json;
use unityrs_testkit::scratch::Scratch;

pub(crate) fn bits(x: f32) -> String {
    format!("{:08x}", x.to_bits())
}

fn id(path: i64) -> J {
    json!({"bundle":0,"file_index":2,"file":CAB,"path_id":path.to_string()})
}

pub(crate) fn budget(prop: &str, target: usize, kind: &str, range: &J) -> J {
    let channel = if prop == "weight" { 0 } else { -1 };
    let mut out = json!({"property":prop,"target":target,"channel":channel,
        "track_kind":kind,"stages":[{"stage":kind,"range":range}]});
    if prop == "weight" {
        out["weight_units"] = json!("editor");
    }
    if prop == "rotation" {
        out["stages"][0]["norm_ranges"] = json!([[[0, 0], [0, 0], [0, 0], [1, 1]]]);
    }
    out
}

fn probe(sample: usize, value: f32) -> J {
    json!({"sample":sample,"property":"weight","target":0,"channel":0,"component":0,"value":bits(value)})
}

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn manifest(pin: &str, legacy: bool) -> J {
    let local = [0., f64::from(0.25_f32.next_down()), 0.5, 1.];
    let zero = bits(0.);
    let one = bits(1.);
    let root = json!({"translation":[zero,zero,zero],"rotation":[zero,zero,zero,one],"scale":[one,one,one]});
    let rest = json!({"transforms":[root,root],"weights":[{"channels":[bits(10.)]},{"channels":[bits(30.)]}]});
    let samples = local.iter().enumerate().map(|(i,&local)| {
        // Closed-form synthetic observations, independent of the reader and sampler.
        let x = if legacy { (local*local*(3.-2.*local)) as f32 } else { 1. };
        let mut pose = rest.clone();
        pose["transforms"][0]["translation"] = json!([bits(x),bits(-2.*x),zero]);
        if legacy {
            let t = local*local*(3.-2.*local);
            let z = f64::from(0.6_f32)*t;
            let w = 1.+(f64::from(0.8_f32)-1.)*t;
            let norm = z.hypot(w);
            pose["transforms"][0]["rotation"] = json!([zero,zero,bits((z/norm) as f32),bits((w/norm) as f32)]);
        }
        pose["weights"][0]["channels"][0] = json!(bits(if i < 2 { 12.5 } else { 17.5 }));
        json!({"local":local,"local_f32":bits(local as f32),"source":2.+local,"effective_source":2.+local,
            "sides":[if i<2 {"left"} else {"right"}],"muscles_f32":[],"pose":pose})
    }).collect::<Vec<_>>();
    let translation = if legacy {
        budget(
            "translation",
            0,
            "hermite",
            &json!([[0, 0], [-2, 1], [0, 0], [0, 0], [1, 1]]),
        )
    } else {
        budget("translation", 0, "constant", &json!([[-2, 1]]))
    };
    let mut rotation = if legacy {
        budget(
            "rotation",
            0,
            "hermite",
            &json!([[0, 1], [0, 1], [0, 0], [0, 0], [1, 1]]),
        )
    } else {
        budget("rotation", 0, "constant", &json!([[0, 1]]))
    };
    if legacy {
        rotation["stages"][0]["norm_ranges"] =
            json!([[[0, 0], [0, 0], [-0.01, 0.61], [0.79, 1.01]]]);
    }
    let fields = if legacy {
        ["key_bounds"; 2]
    } else {
        ["m_MuscleClip.m_StartTime", "m_MuscleClip.m_StopTime"]
    };
    let renderers = [("", 6), ("0", 8)].map(|(address, id_value)| {
        json!({
            "address":address,"id":id(id_value),"component":0,"mesh":id(7),
            "channels":[{"index":0,"name":"bend","full_weights":[25,80]}]
        })
    });
    json!({
        "format":"unityrs.anim_replay","version":1,
        "spec":{
            "bundles":[{"name":"synthetic","sha256":pin}],"build_target":"StandaloneLinux64",
            "prefab":id(1),"prefab_asset":PREFAB,"clip":id(9),"clip_asset":CLIP,
            "clip_name":"explicit fixture","binding":id(3),"binding_address":"","binding_component":0,
            "avatar":null,"avatar_asset":"","mode":if legacy {"legacy"} else {"animator"},
            "start":2,"stop":3,"sample_rate":4,"source_clock":"widened","held_boundaries":[2.25],
            "schedule": local.map(|local| json!({"local":local})),
            "nodes":[{"address":"","name":"root","id":id(2)},{"address":"0","name":"instance","id":id(5)}],
            "renderers":renderers,
            "budgets":[translation,rotation,budget("scale",0,"rest",&json!([[1,1]])),
                budget("translation",1,"rest",&json!([[0,0]])),budget("rotation",1,"rest",&json!([[0,1]])),
                budget("scale",1,"rest",&json!([[1,1]])),budget("weight",0,"held",&json!([[12.5,17.5]])),
                budget("weight",1,"rest",&json!([[30,30]]))],
            "evidence":{"layout":if legacy {"explicit"} else {"streamed_constant"},
                "discovery":[{"owner":id(if legacy {3} else {10}),
                    "field":if legacy {"m_Animation"} else {"m_AnimationClips"},
                    "ordinal":0,"original":id(9),"resolved":id(9),"outcome":"resolved"}],
                "import_settings":[{"property":"loopTime","effective":false,
                    "serialized_path":"m_MuscleClip.m_LoopTime","serialized":if legacy {J::Null} else {json!(false)}}],
                "clock":{
                    "start":{"source":2,"field":fields[0],"probe":probe(0,12.5)},
                    "stop":{"source":3,"field":fields[1],"probe":probe(3,17.5)},
                    "key_times":[2,2.25,3],"held":[{"source":2.25,"left":probe(1,12.5),"right":probe(2,17.5)}]
                }
            }
        },
        "capture":{
            "editor_version":"2022.3.0f1","build_target":"StandaloneLinux64","bundle_sha256":[pin],
            "rest":rest,"samples":samples,
            "clip_settings":{"startTime":zero,"stopTime":zero,"orientationOffsetY":zero,"level":zero,
                "cycleOffset":zero,"additiveReferencePoseTime":zero,"additiveReferencePoseClipName":"",
                "loopTime":false,"loopBlend":false,"loopBlendOrientation":false,"loopBlendPositionY":false,
                "loopBlendPositionXZ":false,"keepOriginalOrientation":false,"keepOriginalPositionY":false,
                "keepOriginalPositionXZ":false,"heightFromFeet":false,"mirror":false,"hasAdditiveReferencePose":false},
            "playback":{"api":if legacy {"Animation.Sample"} else {"AnimationClipPlayable"},
                "wrap_mode":if legacy {"ClampForever"} else {"Default"},"clip_looping":false,
                "clip_human_motion":false,"reset":"fresh_prefab","speed":1,"delta_time":0,
                "apply_root_motion":false,"foot_ik":false,"playable_ik":false,"animator_move":false,
                "controller_blending":false,"events":false,
                "legacy_state":if legacy {"enabled_weight_one_clamp_forever"} else {"unused"},
                // 2022.3 reports a late-start Mecanim clip's length as its stop.
                "clip_length_f32":if legacy {one} else {bits(3.)},"frame_rate_f32":bits(4.)
            }
        }
    })
}

pub(crate) fn with_fixture(legacy: bool, action: impl FnOnce(&Path, &J, &J)) {
    with_recording(legacy, bundle(legacy), |_| {}, action);
}

#[test]
fn generic_replay_matches_trs_and_instance_weights() {
    with_fixture(false, |dir, manifest, tolerance| {
        compare(manifest, dir, tolerance).unwrap();
    });
}

#[test]
fn legacy_replay_matches_trs_and_instance_weights() {
    with_fixture(true, |dir, manifest, tolerance| {
        compare(manifest, dir, tolerance).unwrap();
    });
}

#[test]
fn bundle_cross_checks_refuse_incorrect_replay_evidence() {
    with_fixture(false, |dir, valid, tolerance| {
        let cases = [
            (
                "/spec/evidence/clock/key_times",
                json!([2, 3]),
                "bundle key-time inventory",
            ),
            ("/spec/start", json!(1), "bundle source origin"),
            (
                "/spec/evidence/clock/stop/field",
                json!("m_StartTime"),
                "bundle endpoint fields",
            ),
            (
                "/spec/evidence/clock/stop/source",
                json!(2),
                "bundle endpoint fields",
            ),
            (
                "/spec/budgets/0/stages/0/range",
                json!([[0, 1]]),
                "bundle stage range",
            ),
            (
                "/spec/budgets/1/stages/0/norm_ranges",
                json!([[[1, 1], [0, 0], [0, 0], [0, 0]]]),
                "bundle rotation output boxes",
            ),
            (
                "/spec/budgets/0/track_kind",
                json!("rest"),
                "bundle track kind",
            ),
            (
                "/spec/evidence/clock/held/0/right/value",
                json!(bits(12.5)),
                "held probe disagrees with bundle",
            ),
            (
                "/spec/bundles/0/sha256",
                json!("0".repeat(64)),
                "bundle digest mismatch",
            ),
            (
                "/capture/samples/1/pose/transforms/0/translation/0",
                json!(bits(2.)),
                "worst error",
            ),
            (
                "/spec/evidence/humanoid",
                json!({}),
                "unsupported_humanoid_playback",
            ),
        ];
        for (path, bad, expected) in cases {
            let mut manifest = valid.clone();
            if let Some(old) = manifest.pointer_mut(path) {
                *old = bad;
            } else {
                manifest["spec"]["evidence"]["humanoid"] = bad;
            }
            if path.ends_with("/track_kind") {
                manifest["spec"]["budgets"][0]["stages"][0]["stage"] = json!("rest");
            }
            let error = compare(&manifest, dir, tolerance).unwrap_err();
            let expected = if expected == "worst error" {
                "translation source"
            } else {
                expected
            };
            assert!(error.to_string().contains(expected), "{path}: {error}");
        }
    });
}

#[test]
fn a_request_that_rounds_onto_a_bundle_key_is_not_an_off_key_probe() {
    with_fixture(false, |dir, valid, tolerance| {
        let mut manifest = valid.clone();
        let mut tolerance = tolerance.clone();
        // The only request inside the clip is strictly below the held key
        // in source seconds, but f32 local time rounds to that key.
        let interior = 0.25 - 2f64.powi(-28);
        manifest["spec"]["schedule"] = json!([{"local":0},{"local":interior},{"local":1}]);
        let samples = manifest["capture"]["samples"].as_array_mut().unwrap();
        samples.remove(1);
        samples[1]["local"] = json!(interior);
        samples[1]["local_f32"] = json!(bits(0.25));
        samples[1]["source"] = json!(2. + interior);
        samples[1]["effective_source"] = json!(2.25);
        let samples = tolerance["samples"].as_array_mut().unwrap();
        samples.remove(1);
        samples[1]["source"] = json!(2. + interior);
        samples[1]["effective_source"] = json!(2.25);
        let error = compare(&manifest, dir, &tolerance).unwrap_err();
        assert!(
            error.to_string().contains("bundle interior off-key probe"),
            "{error}"
        );
    });
}

pub(crate) fn rejects(dir: &Path, manifest: &J, tolerance: &J, expected: &str) {
    let error = compare(manifest, dir, tolerance).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "expected {expected}, got {error}"
    );
}

fn reject_changes(dir: &Path, valid: &J, tolerance: &J, changes: &[(&str, J, &str)]) {
    for (path, value, expected) in changes {
        let mut manifest = valid.clone();
        *manifest.pointer_mut(path).unwrap() = value.clone();
        rejects(dir, &manifest, tolerance, expected);
    }
}

#[test]
fn replay_refuses_clock_and_held_evidence_mismatches() {
    with_fixture(false, |dir, valid, tolerance| {
        reject_changes(
            dir,
            valid,
            tolerance,
            &[
                ("/spec/held_boundaries", json!([]), "bundle held boundaries"),
                (
                    "/spec/evidence/clock/held",
                    json!([]),
                    "held probe coverage",
                ),
                (
                    "/spec/evidence/clock/held",
                    json!([
                        valid["spec"]["evidence"]["clock"]["held"][0],
                        valid["spec"]["evidence"]["clock"]["held"][0],
                    ]),
                    "held probe coverage",
                ),
                (
                    "/capture/samples/1/local",
                    json!(0.25),
                    "bundle sample clocks",
                ),
                (
                    "/capture/samples/1/local_f32",
                    json!(bits(0.25)),
                    "bundle sample clocks",
                ),
                (
                    "/capture/samples/1/source",
                    json!(2.25),
                    "bundle sample clocks",
                ),
                (
                    "/capture/samples/1/effective_source",
                    json!(2.25),
                    "bundle sample clocks",
                ),
            ],
        );
        let mut manifest = valid.clone();
        let mut frozen = tolerance.clone();
        manifest["capture"]["samples"][1]["sides"] = json!(["right"]);
        frozen["samples"][1]["sides"] = json!(["right"]);
        rejects(dir, &manifest, &frozen, "bundle held sample sides");

        let mut frozen = tolerance.clone();
        frozen["clock_discriminated"] = json!(false);
        rejects(
            dir,
            valid,
            &frozen,
            "bundle held probe did not distinguish clocks",
        );

        // Keep the held probe on the left under both models, while retaining
        // a valid off-key sample and a frozen claim of discrimination. The
        // f32 sum drops the local time's last bit, so the clocks differ.
        let local = 0.125 + 2f64.powi(-25);
        let (manifest, frozen) = move_interior(valid, tolerance, local, 2. + local);
        rejects(
            dir,
            &manifest,
            &frozen,
            "bundle held probe did not distinguish clocks",
        );
        let mut frozen = frozen;
        frozen["clock_discriminated"] = json!("independent");
        rejects(
            dir,
            &manifest,
            &frozen,
            "bundle held probe did not distinguish clocks",
        );
    });
}

#[test]
fn replay_refuses_budget_stage_path_and_weight_unit_mismatches() {
    with_fixture(false, |dir, valid, tolerance| {
        reject_changes(
            dir,
            valid,
            tolerance,
            &[
                (
                    "/spec/budgets/0/stages/0/stage",
                    json!("held"),
                    "bundle track stage path",
                ),
                (
                    "/spec/budgets/6/weight_units",
                    json!("fraction_of_100"),
                    "bundle weight stage units",
                ),
            ],
        );
    });
}

#[test]
fn replay_refuses_identity_mismatches() {
    with_fixture(false, |dir, valid, tolerance| {
        reject_changes(
            dir,
            valid,
            tolerance,
            &[
                ("/spec/nodes/0/id", id(5), "node identity"),
                ("/spec/renderers/0/id", id(8), "renderer identity"),
                ("/spec/renderers/0/mesh", id(6), "mesh identity"),
                (
                    "/spec/renderers/0/channels/0/name",
                    json!("other"),
                    "renderer channel identity",
                ),
                (
                    "/spec/prefab_asset",
                    json!("assets/wrong.prefab"),
                    "bundle asset key",
                ),
                ("/spec/binding", id(10), "binding identity"),
                ("/spec/clip_name", json!("other"), "recorded clip name"),
                (
                    "/spec/evidence/import_settings/0/serialized",
                    json!(true),
                    "bundle serialized import setting",
                ),
                ("/spec/mode", json!("legacy"), "missing recorded binding"),
            ],
        );
    });
    let mut clip = generic_clip(GenericClip::ConstantTrs);
    unityrs_testkit::scene3d::set(&mut clip.value, "m_Legacy", V::Bool(true));
    with_clip(
        false,
        clip,
        |_| {},
        |dir, valid, frozen| rejects(dir, valid, frozen, "serialized clip playback mode"),
    );
}

#[test]
fn replay_refuses_mismatched_rest_rotation_and_instance_weight_samples() {
    with_fixture(false, |dir, valid, tolerance| {
        reject_changes(
            dir,
            valid,
            tolerance,
            &[
                (
                    "/capture/rest/transforms/0/translation/0",
                    json!(bits(0.5)),
                    "serialized rest pose mismatch",
                ),
                (
                    "/capture/samples/2/pose/transforms/0/rotation",
                    json!([bits(0.), bits(0.), bits(1.), bits(0.)]),
                    "rotation source",
                ),
                (
                    "/capture/samples/2/pose/transforms/0/rotation",
                    json!(vec![bits(0.); 4]),
                    "zero-norm captured quaternion",
                ),
            ],
        );
        let mut swapped = valid.clone();
        swapped["capture"]["samples"][2]["pose"]["weights"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        rejects(dir, &swapped, tolerance, "weight source");
    });
}

#[test]
fn legacy_replay_requires_key_bounds_and_hermite_ranges() {
    with_fixture(true, |dir, valid, tolerance| {
        reject_changes(
            dir,
            valid,
            tolerance,
            &[
                (
                    "/spec/evidence/clock/start/field",
                    json!("m_MuscleClip.m_StartTime"),
                    "bundle endpoint fields",
                ),
                (
                    "/spec/evidence/clock/stop/source",
                    json!(2.75),
                    "bundle endpoint fields",
                ),
                (
                    "/spec/budgets/0/stages/0/range/1",
                    json!([0, 1]),
                    "bundle stage range",
                ),
            ],
        );
    });
}

use crate::anim_support::{GenericClip, bundle_with_clip, generic_clip, legacy_clip};
use unityrs_testkit::{
    ngui::{V, vector},
    scene3d::anim::playback::{self as p, Key},
};

pub(crate) fn with_clip(
    legacy: bool,
    clip: unityrs_testkit::ngui::Object,
    edit: impl FnOnce(&mut J),
    action: impl FnOnce(&Path, &J, &J),
) {
    let bytes = bundle_with_clip(legacy, clip);
    with_recording(legacy, bytes, edit, action);
}

fn with_recording(
    legacy: bool,
    bytes: Vec<u8>,
    edit: impl FnOnce(&mut J),
    action: impl FnOnce(&Path, &J, &J),
) {
    let scratch = Scratch::new("anim-synthetic-replay");
    let mut document = manifest(&digest(&bytes), legacy);
    edit(&mut document);
    let path = scratch.join("capture.json");
    std::fs::write(scratch.join("synthetic"), bytes).unwrap();
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let tolerance = derive(&path, scratch.path()).unwrap();
    action(scratch.path(), &document, &tolerance);
}

#[test]
fn replay_refuses_a_mixed_stage_track() {
    let mut clip = legacy_clip();
    let mut a = Key::new(2., [0.; 3]);
    a.outgoing = [f32::INFINITY, 0., 0.];
    let row = p::curve(
        "",
        &[a, Key::new(3., [0., -2., 0.])],
        [2, 2],
        None,
        true,
        None,
    );
    unityrs_testkit::scene3d::set(
        &mut clip.value,
        "m_PositionCurves",
        vector(row.clone(), vec![row]),
    );
    // The declared Hermite-only path cannot describe a held X and Hermite Y.
    with_clip(
        true,
        clip,
        |_| {},
        |dir, valid, frozen| {
            rejects(dir, valid, frozen, "bundle mixed-stage track kind");
        },
    );
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn legacy_euler_replay_requires_joint_input_range() {
    with_clip(
        true,
        legacy_euler_clip(&[Key::new(2., [0.; 3]), Key::new(3., [0., 0., 45.])]),
        |document| {
            let mut rotation = budget(
                "rotation",
                0,
                "euler_hermite",
                &json!([[0, 0], [0, 45], [0, 0], [0, 0], [1, 1]]),
            );
            rotation["stages"] = json!([
                {"stage":"hermite","range":rotation["stages"][0]["range"]},
                {"stage":"joint","range":[[-10,55]],"norm_ranges":[[[-0.01,0.01],[-0.01,0.01],[-0.01,0.4],[0.9,1.01]]]}
            ]);
            document["spec"]["budgets"][1] = rotation;
            for sample in document["capture"]["samples"].as_array_mut().unwrap() {
                let t = sample["local"].as_f64().unwrap();
                let angle = std::f64::consts::FRAC_PI_8 * t * t * (3. - 2. * t);
                sample["pose"]["transforms"][0]["rotation"] = json!([
                    bits(0.),
                    bits(0.),
                    bits(angle.sin() as f32),
                    bits(angle.cos() as f32)
                ]);
            }
        },
        |dir, valid, frozen| {
            compare(valid, dir, frozen).unwrap();
            reject_changes(
                dir,
                valid,
                frozen,
                &[(
                    "/spec/budgets/1/stages/1/range/0",
                    json!([0, 1]),
                    "bundle Euler stage range",
                )],
            );
        },
    );
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn streamed_stage_range_covers_held_final_key() {
    with_clip(
        false,
        generic_clip(GenericClip::ClampedTranslation),
        |document| {
            document["spec"]["evidence"]["clock"]["key_times"] = json!([2, 2.25, 2.75, 3]);
            document["spec"]["budgets"][0] = budget(
                "translation",
                0,
                "streamed",
                &json!([[0, 0], [0, 0], [1, 1], [0, 1000], [0, 0.75]]),
            );
            for sample in document["capture"]["samples"].as_array_mut().unwrap() {
                let local = sample["local"].as_f64().unwrap();
                let x = if local < 0.75 { local as f32 } else { 1000. };
                sample["pose"]["transforms"][0]["translation"] =
                    json!([bits(x), bits(0.), bits(0.)]);
            }
        },
        |dir, valid, frozen| {
            compare(valid, dir, frozen).unwrap();
            reject_changes(
                dir,
                valid,
                frozen,
                &[(
                    "/spec/budgets/0/stages/0/range/3",
                    json!([0, 1]),
                    "bundle stage range",
                )],
            );
        },
    );
}

fn legacy_euler_clip(keys: &[Key<3>]) -> unityrs_testkit::ngui::Object {
    let mut clip = legacy_clip();
    let V::Struct(_, fields) = &mut clip.value else {
        unreachable!()
    };
    fields.retain(|(name, _)| name != "m_RotationCurves");
    let row = p::curve("", keys, [2, 2], Some(4), true, None);
    unityrs_testkit::scene3d::set(
        &mut clip.value,
        "m_EulerCurves",
        vector(row.clone(), vec![row]),
    );
    clip
}

#[test]
fn held_rotation_probes_accept_the_opposite_quaternion_sign() {
    let mut a = Key::new(2., [0.; 3]);
    a.outgoing = [f32::INFINITY; 3];
    let mut b = Key::new(2.25, [0., 0., 90.]);
    b.outgoing = [f32::INFINITY; 3];
    let component = -std::f32::consts::FRAC_1_SQRT_2;
    with_clip(
        true,
        legacy_euler_clip(&[a, b, Key::new(3., b.value)]),
        |document| {
            document["spec"]["budgets"][1] = json!({
                "property":"rotation","target":0,"channel":-1,"track_kind":"euler_held",
                "stages":[{"stage":"held","range":[[0,90]]},
                    {"stage":"joint","range":[[0,90]],"norm_ranges":[[[0,0],[0,0],[0,0.71],[0.70,1]]]}]
            });
            let held = &mut document["spec"]["evidence"]["clock"]["held"][0];
            for (side, sample, value) in [("left", 1, 0.), ("right", 2, component)] {
                held[side] = json!({"sample":sample,"property":"rotation","target":0,
                "channel":-1,"component":2,"value":bits(value)});
            }
            for (i, sample) in document["capture"]["samples"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                sample["pose"]["transforms"][0]["rotation"] = if i < 2 {
                    json!([bits(0.), bits(0.), bits(0.), bits(-1.)])
                } else {
                    json!([bits(0.), bits(0.), bits(component), bits(component)])
                };
            }
        },
        |dir, valid, frozen| compare(valid, dir, frozen).unwrap(),
    );
}

#[test]
fn held_quaternion_sign_flip_does_not_require_a_clock_probe() {
    with_clip(
        false,
        generic_clip(GenericClip::HeldQuaternionFlip),
        |document| {
            // The held weight jump at 2.25 still distinguishes the source clocks.
            document["spec"]["evidence"]["clock"]["key_times"] = json!([2, 2.25, 2.75, 3]);
            let mut rotation = budget("rotation", 0, "held", &json!([[-1, 1]]));
            rotation["stages"][0]["norm_ranges"] = json!([
                [[0, 0], [0, 0], [0, 0], [1, 1]],
                [[0, 0], [0, 0], [0, 0], [-1, -1]]
            ]);
            document["spec"]["budgets"][1] = rotation;
            for (i, sample) in document["capture"]["samples"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                sample["pose"]["transforms"][0]["rotation"] = json!([
                    bits(0.),
                    bits(0.),
                    bits(0.),
                    bits(if i == 3 { -1. } else { 1. })
                ]);
            }
        },
        |dir, valid, frozen| compare(valid, dir, frozen).unwrap(),
    );
}

#[test]
fn f32_add_collisions_keep_probe_indices_and_check_each_pose() {
    with_recording(
        false,
        bundle(false),
        |document| {
            let clock = &mut document["spec"]["evidence"]["clock"];
            clock["held"][0]["left"] = probe(0, 12.5);
            clock["held"][0]["right"] = probe(1, 17.5);
            clock["stop"]["probe"]["sample"] = json!(4);
            document["spec"]["source_clock"] = json!("f32_add");
            document["spec"]["schedule"]
                .as_array_mut()
                .unwrap()
                .insert(2, json!({"local":0.25}));
            let samples = document["capture"]["samples"].as_array_mut().unwrap();
            samples[1]["effective_source"] = json!(2.25);
            samples[1]["sides"] = json!(["right"]);
            samples[1]["pose"]["weights"][0]["channels"][0] = json!(bits(17.5));
            // Y sets the output rounding bound. The neighboring X values
            // fit individually, but their separation exceeds either bound.
            samples[1]["pose"]["transforms"][0]["translation"][0] = json!(bits(1_f32.next_up()));
            let mut same_time = samples[1].clone();
            same_time["local"] = json!(0.25);
            same_time["local_f32"] = json!(bits(0.25));
            same_time["source"] = json!(2.25);
            same_time["pose"]["transforms"][0]["translation"][0] = json!(bits(1_f32.next_down()));
            samples.insert(2, same_time);
        },
        |dir, valid, frozen| {
            compare(valid, dir, frozen).unwrap();
            reject_changes(
                dir,
                valid,
                frozen,
                &[
                    (
                        "/capture/samples/1/pose/transforms/0/translation/0",
                        json!(bits(2.)),
                        "translation source 2.25:",
                    ),
                    (
                        "/capture/samples/2/pose/transforms/0/translation/0",
                        json!(bits(2.)),
                        "translation source 2.25:",
                    ),
                ],
            );
        },
    );
}
