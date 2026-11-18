//! Synthetic bundle, Editor-shaped observations and predeclared ranges.
use crate::anim_support::{CAB, CLIP, PREFAB, bundle};
use crate::{J, Path, compare, derive, digest};
use serde_json::json;
use unityrs_testkit::scratch::Scratch;

fn bits(x: f32) -> String {
    format!("{:08x}", x.to_bits())
}

fn id(path: i64) -> J {
    json!({"bundle":0,"file_index":2,"file":CAB,"path_id":path.to_string()})
}

fn budget(prop: &str, target: usize, kind: &str, range: &J) -> J {
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
                "clip_length_f32":one,"frame_rate_f32":bits(4.)
            }
        }
    })
}

fn with_fixture(legacy: bool, action: impl FnOnce(&Path, &J, &J)) {
    let scratch = Scratch::new("anim-synthetic-replay");
    let bytes = bundle(legacy);
    let document = manifest(&digest(&bytes), legacy);
    let path = scratch.join("capture.json");
    std::fs::write(scratch.join("synthetic"), bytes).unwrap();
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let tolerance = derive(&path, scratch.path()).unwrap();
    action(scratch.path(), &document, &tolerance);
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
