use super::{drive, head, job, rows};
use serde_json::{Value, json};
use std::fs;
use unityrs_batch::{
    digest::{Hashes, Hex},
    reason::Code,
};
use unityrs_core::{
    RawStr,
    env::{EnvOptions, Environment, Source},
};
use unityrs_scene::{GraphOptions, Unit};
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec},
    env,
    ngui::{self, Object, V, st, vector},
    scene3d::{
        self as s, Layout,
        anim::{
            self,
            playback::{self as p, Key},
        },
    },
};

pub(super) fn catalog(paths: &[(&str, i64)]) -> Vec<u8> {
    env::serialized(
        &["CAB-rig"],
        &[env::Obj::asset_bundle(
            1,
            env::Bundle {
                rows: paths
                    .iter()
                    .map(|&(name, id)| env::row(name, (0, 0), (1, id)))
                    .collect(),
                ..Default::default()
            },
        )],
    )
}

#[derive(Clone, Copy)]
enum Clip {
    Translation,
    NoTracks,
    TooLong,
    Outside,
    MissingMesh,
    Backref,
    BadStaticRange,
    BadStaticRangeWithMesh,
    StreamedMesh,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Material {
    Missing,
    Streamed,
    Large,
}

fn fixture(clip: Clip, material: bool) -> (Vec<u8>, Vec<u8>) {
    build_fixture(clip, material.then_some(Material::Missing), true)
}

fn rig_bundle(clip: Clip, material: bool, texture: bool) -> Vec<u8> {
    let mut rig = vec![
        Object::new(
            1,
            1,
            s::game_object(
                b"root",
                if matches!(clip, Clip::BadStaticRangeWithMesh) {
                    &[(0, 2), (0, 3), (0, 4), (0, 5), (0, 6)]
                } else {
                    &[(0, 2), (0, 3), (0, 4), (0, 5)]
                },
            ),
        ),
        Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
        anim::animator(3, (1, 10)),
        Object::new(4, 33, s::mesh_filter((0, 1), (1, 7))),
        Object::new(
            5,
            23,
            s::mesh_renderer(
                (0, 1),
                if material { &[(1, 8)] } else { &[] },
                if matches!(clip, Clip::BadStaticRange | Clip::BadStaticRangeWithMesh) {
                    (1, 1)
                } else {
                    (0, 0)
                },
            ),
        ),
        Object::new(11, 1, s::game_object(b"second", &[(0, 12), (0, 13)])),
        Object::new(12, 4, s::transform((0, 11), (0, 0), &[], false)),
        anim::animator(13, (1, 10)),
    ];
    if matches!(clip, Clip::BadStaticRangeWithMesh) {
        rig.push(Object::new(6, 23, s::mesh_renderer((0, 1), &[], (0, 0))));
    }
    if texture {
        rig.push(ngui::texture(14, b"pixel", 4, (1, 1), vec![255; 4]));
    }
    if matches!(clip, Clip::Backref) {
        rig.push(p::explicit(
            9,
            4.,
            &[("m_PositionCurves", vec![position(1.)])],
        ));
    }
    // Roots are stored under a separate AssetBundle index, just as for a
    // bundle that has more than one CAB.
    let rows = if matches!(clip, Clip::StreamedMesh) {
        // Seal a successful candidate before the stream is read.
        [("second", 11), ("first", 1)]
    } else {
        [("first", 1), ("second", 11)]
    };
    env::bundle(&[
        ("CAB-index", catalog(&rows)),
        (
            "CAB-rig",
            s::file(Layout::Unity2022, &["CAB-objects"], &rig).unwrap(),
        ),
    ])
}

fn build_fixture(clip: Clip, material: Option<Material>, texture: bool) -> (Vec<u8>, Vec<u8>) {
    let input = rig_bundle(clip, material.is_some(), texture);
    let backref = matches!(clip, Clip::Backref);
    let end = if matches!(clip, Clip::TooLong) {
        1000.
    } else {
        1.
    };
    let fields = [("m_PositionCurves", vec![position(end)])];
    let use_outside = matches!(clip, Clip::Outside);
    let mut objects = vec![
        anim::controller(10, &[(i32::from(use_outside || backref), 9)], &[0]),
        p::explicit(
            9,
            4.,
            if matches!(clip, Clip::NoTracks) {
                &[]
            } else {
                &fields
            },
        ),
    ];
    if !matches!(clip, Clip::MissingMesh) {
        let mut mesh = triangle();
        if matches!(clip, Clip::StreamedMesh) {
            s::set(
                &mut mesh.value,
                "m_StreamData",
                st(
                    "StreamingInfo",
                    vec![
                        ("offset", V::UInt64(0)),
                        ("size", V::UInt(64)),
                        ("path", V::Str(b"stream.resS".to_vec())),
                    ],
                ),
            );
        }
        objects.push(mesh);
    }
    add_material(&mut objects, material);
    let streamed = material == Some(Material::Streamed) || matches!(clip, Clip::StreamedMesh);
    let external: &[&str] = if use_outside {
        &["CAB-outside"]
    } else if backref {
        &["CAB-rig"]
    } else {
        &[]
    };
    let source = s::file(Layout::Unity2022, external, &objects).unwrap();
    let deps = if streamed {
        let mut source = source;
        source.resize(600 << 10, 0);
        large_bundle(b"CAB-objects", source, 4)
    } else {
        env::bundle(&[("CAB-objects", source)])
    };
    (input, deps)
}

fn add_material(objects: &mut Vec<Object>, material: Option<Material>) {
    if material.is_some() {
        objects.push(s::material(
            8,
            b"factor",
            (0, 0),
            &[],
            &[("_Color", [0.4, 0.5, 0.6, 1.])],
            &[("_MainTex", s::tex_env((0, 99), [1.; 2], [0.; 2]))],
        ));
    }
    if material == Some(Material::Streamed) {
        objects.push(ngui::texture_streamed(
            99,
            b"optional",
            4,
            (1, 1),
            "stream.resS",
            0,
            32,
        ));
    }
    if material == Some(Material::Large) {
        objects.push(ngui::texture(
            99,
            b"large",
            10,
            (768, 768),
            vec![0; 768 * 768 / 2],
        ));
    }
}

fn position(end: f32) -> V {
    p::curve(
        "",
        &[Key::new(0., [0.; 3]), Key::new(end, [1., 2., 3.])],
        [2, 2],
        None,
        true,
        None,
    )
}

fn large_bundle(name: &[u8], data: Vec<u8>, flags: u32) -> Vec<u8> {
    bundle::build(&BundleSpec {
        block_size: data.len(),
        entries: vec![EntrySpec {
            name: name.to_vec(),
            data,
            flags,
        }],
        ..Default::default()
    })
}

pub(super) fn triangle() -> Object {
    let vertices = [
        [0., 0., 0., 0., 0.],
        [1., 0., 0., 1., 0.],
        [0., 1., 0., 0., 1.],
    ];
    let mut mesh = s::mesh::mesh(
        7,
        b"triangle",
        &[[0.; 3], [1., 0., 0.], [0., 1., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    );
    let channel = |offset, dimension| {
        st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(offset)),
                ("format", V::UInt8(0)),
                ("dimension", V::UInt8(dimension)),
            ],
        )
    };
    let mut data: Vec<u8> = vertices
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    data.resize(data.len().next_multiple_of(16), 0);
    s::set(
        &mut mesh.value,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(3)),
                (
                    "m_Channels",
                    vector(
                        channel(0, 0),
                        vec![
                            channel(0, 3),
                            channel(0, 0),
                            channel(0, 0),
                            channel(0, 0),
                            channel(12, 2),
                        ],
                    ),
                ),
                ("m_DataSize", V::Bytes(data)),
            ],
        ),
    );
    mesh
}

fn install(
    scratch: &unityrs_testkit::scratch::Scratch,
    spec: &mut Value,
    input: &[u8],
    deps: &[u8],
) {
    fs::write(scratch.join("one.bundle"), input).unwrap();
    fs::write(scratch.join("deps.bundle"), deps).unwrap();
    spec["inputs"][0]["deps"] = json!(["deps"]);
    spec["dependencies"] = json!([{"id":"deps","path":scratch.join("deps.bundle")}]);
}

fn index(file_index: u32, path_id: i64) -> Value {
    json!({"file_index":file_index,"path_id":path_id})
}

fn selection(root: i64, component: i64) -> Value {
    json!({
        "root":index(2, root),"component":index(2, component),
        "source":{"owner":index(4,10),"field":"m_AnimationClips","ordinal":0}
    })
}

fn standalone(input: &[u8], deps: &[u8], root: i64, clips: &Value) -> Vec<u8> {
    let env = Environment::new(EnvOptions::unitypy());
    let load = |name, bytes: &[u8]| {
        env.load(Source::Bytes {
            name: RawStr::from_static(name),
            data: bytes.to_vec().into(),
        })
        .unwrap()
    };
    let roots = [load(b"input", input), load(b"deps", deps)];
    let root = env
        .objects()
        .find_map(|row| {
            row.ok()
                .filter(|o| o.class_id() == 1 && o.path_id() == root)
        })
        .unwrap();
    let unit = Unit::Prefab(root);
    let meter = unit.meter();
    let graph = unit.graph(&env, &GraphOptions::default(), meter).unwrap();
    let profile = unityrs_gltf::Profile::parse(
        &serde_json::to_vec(&json!({"profile_version":1, "clips":clips})).unwrap(),
        meter,
    )
    .unwrap();
    let collected = unityrs_export::gltf::collect(
        &env,
        &roots,
        &graph,
        &profile,
        &unityrs_gltf::UnlitExtras,
        meter,
    )
    .unwrap();
    let mut output = Vec::new();
    unityrs_gltf::write_glb(&collected.scene, &profile.options(), meter, &mut output).unwrap();
    output
}

#[test]
fn cross_bundle_animation_and_mesh_match_standalone_for_both_rigs() {
    for threads in [1, 3] {
        let (scratch, mut spec) = job(&[]);
        let (input, deps) = fixture(Clip::Translation, false);
        install(&scratch, &mut spec, &input, &deps);
        spec["threads"] = json!(threads);
        spec["outputs"]["textures"] = json!({"key":"{name}.png"});
        spec["outputs"]["glb"]["profile"] = json!({
            "profile_version":1, "clips_by_prefab":{"second.glb":{"a":[selection(12,13)]}}
        });
        assert_eq!(drive(&scratch, &spec).code, Code::Complete);
        for (key, id, clips) in [
            ("first.glb", 1, json!("all_discovered")),
            ("second.glb", 11, json!([selection(12, 13)])),
        ] {
            let got = fs::read(scratch.join(format!("sink/{key}"))).unwrap();
            let want = standalone(&input, &deps, id, &clips);
            assert_eq!(got, want);
            let record = rows(&scratch, 1, "outputs")
                .into_iter()
                .find(|r| r["key"] == key)
                .unwrap();
            assert_eq!(record["md5"], Hex(&Hashes::of(&want).md5).to_string());
        }
        assert!(scratch.join("sink/pixel.png").exists());
    }
}

#[test]
fn explicit_outside_view_keeps_source_slot_and_has_no_candidate() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Outside, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips_by_prefab":{"first.glb":{"a":[selection(2,3)]}}
    });
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips[0]["reason"], "outside_view(CAB-outside)");
    assert!(
        skips[0]["detail"]
            .as_str()
            .unwrap()
            .contains("m_AnimationClips[0]")
    );
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[test]
fn empty_clip_and_bake_cap_are_typed_object_skips() {
    for (clip, reason, detail) in [
        (Clip::NoTracks, "no_playable_channels", Value::Null),
        (
            Clip::TooLong,
            "animation_bake_error",
            json!("schedule key cap"),
        ),
    ] {
        let (scratch, mut spec) = job(&[]);
        let (input, deps) = fixture(clip, false);
        install(&scratch, &mut spec, &input, &deps);
        spec["outputs"]["glb"]["profile"] = json!({
            "profile_version":1, "sample_rate":1024, "clips_by_prefab":{"second.glb":{"a":[]}}
        });
        assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
        let skips = rows(&scratch, 1, "skips");
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0]["output"], "glb");
        assert_eq!(skips[0]["reason"], reason);
        assert_eq!(skips[0]["detail"], detail);
        assert_eq!(
            rows(&scratch, 1, "inputs")[0]["status"],
            "complete_with_skips"
        );
        assert!(!scratch.join("sink/first.glb").exists());
        assert!(scratch.join("sink/second.glb").exists());
    }
}

#[test]
fn optional_texture_keeps_candidate_and_records_diagnostic_only_exit() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Translation, true);
    install(&scratch, &mut spec, &input, &deps);
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    assert_eq!(head(&scratch)["run"]["glb_degraded"], 1);
    assert_eq!(head(&scratch)["counts"]["skips"], 0);
    let diagnostics = rows(&scratch, 1, "diagnostics");
    let row = diagnostics
        .iter()
        .find(|r| r["kind"] == "glb_degraded")
        .unwrap();
    assert_eq!(row["key"], "first.glb");
    assert_eq!(row["reason"], "missing_texture");
    assert_eq!(row["renderer_path_id"], 5);
    assert_eq!(row["resource_owner_path_id"], 8);
    assert_eq!(row["resource_file_id"], 0);
    assert_eq!(row["resource_path_id"], 99);
    assert_eq!(rows(&scratch, 1, "inputs")[0]["status"], "complete");
    assert!(scratch.join("sink/first.glb").exists());
}

#[test]
fn selectors_do_not_cross_input_views_and_preference_uses_glb_bytes() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Translation, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["inputs"].as_array_mut().unwrap().push(json!({
        "id":"b","path":scratch.join("one.bundle"),"deps":["deps"]
    }));
    let animated = standalone(&input, &deps, 1, &json!("all_discovered"));
    let static_glb = standalone(&input, &deps, 1, &json!([]));
    assert_ne!(animated, static_glb);
    let expected = Hex(&Hashes::of(&animated).md5).to_string();
    spec["prefer"] = json!({"first.glb":{"md5":expected}});
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips_by_prefab":{"first.glb":{"a":[]}}
    });
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    assert_eq!(fs::read(scratch.join("sink/first.glb")).unwrap(), animated);
    let collisions = rows(&scratch, 1, "collisions");
    let first: Vec<_> = collisions
        .iter()
        .filter(|r| r["key"] == "first.glb")
        .collect();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0]["input"], "a");
    assert_eq!(
        first[0]["md5"],
        Hex(&Hashes::of(&static_glb).md5).to_string()
    );
    assert_eq!(first[0]["winner"], false);
    assert_eq!(first[1]["input"], "b");
    assert_eq!(first[1]["winner"], true);
    assert!(first.iter().all(|row| row["pixels_differ"] == false));
}

#[test]
fn earlier_2d_keys_win_within_input_and_glb_units_win_across_inputs() {
    for within_input in [true, false] {
        let (scratch, mut spec) = job(&[]);
        let (input, deps) = fixture(Clip::Translation, false);
        install(&scratch, &mut spec, &input, &deps);
        spec["outputs"]["textures"] = json!({"key":"first.glb"});
        if !within_input {
            let mut file = s::file(
                Layout::Unity2022,
                &[],
                &[ngui::texture(14, b"pixel", 4, (1, 1), vec![255; 4])],
            )
            .unwrap();
            file = env::bundle(&[("CAB-rig", file)]);
            fs::write(scratch.join("texture.bundle"), file).unwrap();
            let input = spec["inputs"][0].clone();
            spec["inputs"] = json!([{"id":"b","path":scratch.join("texture.bundle")},input]);
            // The view-local 2D claim lives in b alone.
            let (own, _) = build_fixture(Clip::Translation, None, false);
            fs::write(scratch.join("one.bundle"), own).unwrap();
        }
        let end = drive(&scratch, &spec);
        let bytes = fs::read(scratch.join("sink/first.glb")).unwrap();
        if within_input {
            assert_eq!(end.code, Code::CompleteWithSkips);
            assert_eq!(rows(&scratch, 1, "skips")[0]["reason"], "duplicate_key");
            assert_eq!(&bytes[..4], b"\x89PNG");
        } else {
            assert_eq!(end.code, Code::Complete);
            let rows = rows(&scratch, 1, "collisions");
            assert!(rows.iter().all(|row| row["pixels_differ"] == false));
            assert!(
                rows.iter().any(|row| row["input"] == "b"
                    && row["rule"] == "unit"
                    && row["winner"] == false)
            );
            assert_eq!(&bytes[..4], b"glTF");
        }
    }
}

#[test]
fn export_work_refusal_discards_the_attempt_and_obeys_publication_policy() {
    for publish in [true, false] {
        let (scratch, mut spec) = job(&[]);
        let (input, deps) = fixture(Clip::Translation, false);
        install(&scratch, &mut spec, &input, &deps);
        spec["inputs"].as_array_mut().unwrap().push(json!({
            "id":"b","path":scratch.join("one.bundle"),"deps":["deps"]
        }));
        spec["max_export_work"] = json!(1 << 20);
        spec["publish_incomplete"] = json!(publish);
        spec["prefer"] = json!({"first.glb":{"input":"a"}});
        let mut request = selection(12, 13);
        request["source"]["field"] = json!("m_AnimationClips".repeat(9));
        spec["outputs"]["glb"]["profile"] = json!({
            "profile_version":1, "clips":[], "clips_by_prefab":{"second.glb":{"a":vec![request; 150]}}
        });
        assert_eq!(drive(&scratch, &spec).code, Code::Budget);
        let inputs = rows(&scratch, 1, "inputs");
        assert_eq!(inputs[0]["status"], "failed");
        assert_eq!(inputs[0]["reason"], "limit(export_work, 1048576)");
        assert_eq!(inputs[1]["status"], "complete");
        let output = rows(&scratch, 1, "outputs");
        assert_eq!(output[0]["reason"], "withheld");
        assert_eq!(
            output[1]["reason"],
            if publish {
                Value::Null
            } else {
                json!("publish_incomplete")
            }
        );
        assert!(!scratch.join("sink/first.glb").exists());
        assert_eq!(scratch.join("sink/second.glb").exists(), publish);
    }
}

#[test]
fn optional_stream_dependency_budget_discards_the_attempt() {
    stream_budget(Clip::Translation, Some(Material::Streamed));
}

#[test]
fn wrapped_mesh_stream_dependency_budget_discards_the_attempt() {
    stream_budget(Clip::StreamedMesh, None);
}

fn stream_budget(clip: Clip, material: Option<Material>) {
    for publish in [true, false] {
        let (scratch, mut spec) = job(&[]);
        let (input, deps) = build_fixture(clip, material, false);
        install(&scratch, &mut spec, &input, &deps);
        let source = large_bundle(b"stream.resS", vec![0; 700 << 10], 0);
        fs::write(scratch.join("resource.bundle"), source).unwrap();
        spec["dependencies"].as_array_mut().unwrap().push(json!({
            "id":"raw", "path":scratch.join("resource.bundle")
        }));
        spec["inputs"][0]["deps"] = json!(["deps", "raw"]);
        let good = super::fixture(&[("good", 1)]);
        fs::write(scratch.join("good.bundle"), good).unwrap();
        spec["inputs"].as_array_mut().unwrap().push(json!({
            "id":"b","path":scratch.join("good.bundle")
        }));
        spec["max_decoded_bytes"] = json!(1 << 20);
        spec["dep_cache_bytes"] = json!(1 << 20);
        spec["outputs"]["glb"]["profile"] = json!({"profile_version":1, "clips":[]});
        spec["publish_incomplete"] = json!(publish);
        assert_eq!(drive(&scratch, &spec).code, Code::Budget);
        let inputs = rows(&scratch, 1, "inputs");
        assert_eq!(inputs[0]["status"], "failed", "{inputs:?}");
        assert_eq!(inputs[0]["reason"], "budget(dependency)", "{inputs:?}");
        assert_eq!(inputs[1]["status"], "complete");
        assert!(!scratch.join("sink/first.glb").exists());
        assert!(!scratch.join("sink/second.glb").exists());
        assert_eq!(scratch.join("sink/good.glb").exists(), publish);
    }
}

#[test]
fn missing_required_mesh_skips_the_prefab_and_leaves_the_other_eligible() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::MissingMesh, false);
    install(&scratch, &mut spec, &input, &deps);
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0]["reason"], "invalid_glb");
    assert_eq!(skips[0]["detail"], "mesh_unresolved");
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[test]
fn an_export_failure_does_not_reoffer_the_first_owners_key() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::MissingMesh, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["outputs"]["glb"]["prefab_key_template"] = json!("shared.glb");
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 2);
    assert_eq!(skips[0]["container_row"], 1);
    assert_eq!(skips[0]["reason"], "duplicate_key");
    assert_eq!(skips[1]["container_row"], 0);
    assert_eq!(skips[1]["reason"], "invalid_glb");
    assert_eq!(skips[1]["detail"], "mesh_unresolved");
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/shared.glb").exists());
}

#[test]
fn a_resolved_selector_outside_the_view_retains_its_address() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Translation, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips":[], "clips_by_prefab":{
            "first.glb":{"a":[{"root":index(2,2),"component":index(2,3),"clip":index(100,9)}]}
        }
    });
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips[0]["reason"], "outside_view(file_index=100)");
    assert_eq!(skips[0]["detail"], "path_id=9");
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[test]
fn explicit_selection_accepts_an_own_clip_reached_through_a_dependency() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Backref, false);
    install(&scratch, &mut spec, &input, &deps);
    let requested = json!([{"root":index(2,2),"component":index(2,3),"clip":index(2,9)}]);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips":[], "clips_by_prefab":{"first.glb":{"a":requested}}
    });
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    let got = fs::read(scratch.join("sink/first.glb")).unwrap();
    let want = standalone(&input, &deps, 1, &requested);
    assert_eq!(got, want);
}

#[test]
fn input_memory_refusal_skips_only_the_prefab_that_needs_the_allocation() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = build_fixture(Clip::Translation, Some(Material::Large), false);
    install(&scratch, &mut spec, &input, &deps);
    spec["max_decoded_bytes"] = json!(1 << 20);
    spec["max_export_work"] = json!(16 << 20);
    spec["outputs"]["glb"]["profile"] = json!({"profile_version":1, "clips":[]});
    assert_eq!(drive(&scratch, &spec).code, Code::Budget);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0]["reason"], "limit(input_memory, 1048576)");
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[path = "review.rs"]
mod review;
