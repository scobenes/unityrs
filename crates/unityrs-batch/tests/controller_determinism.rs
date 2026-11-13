mod controller_support;
mod stage_support;
use controller_support::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
};
use unityrs_batch::{
    controller::{Boundary, NoHooks},
    spec::model::{Atlases, Outputs},
};
use unityrs_testkit::{
    Rng, guard,
    ngui::{AtlasScene, SceneBundle},
};

type Artifact = (Vec<Value>, BTreeMap<String, Vec<u8>>);
fn run(
    scene: &AtlasScene,
    inputs: &[&str],
    threads: u32,
    slots: u32,
) -> BTreeMap<String, Artifact> {
    let bundles = |included: bool| {
        scene
            .bundles
            .iter()
            .filter(|b| inputs.contains(&b.id.as_str()) == included)
            .map(|b| {
                (
                    b.id.as_str(),
                    b.data.clone(),
                    b.deps.iter().map(String::as_str).collect(),
                )
            })
            .collect::<Vec<_>>()
    };
    let unsorted = bundles(true);
    let input: Vec<_> = inputs
        .iter()
        .map(|id| unsorted.iter().find(|b| b.0 == *id).unwrap().clone())
        .collect();
    let mut h = Harness::new(&input, &bundles(false), threads, slots);
    h.configure(|s| {
        s.outputs = Outputs {
            textures: scene.options.texture_key.clone(),
            sprites: scene.options.sprites_key.clone(),
            atlases: Some(Atlases {
                sprite_key: scene.options.sprite_key.clone(),
                json_key: scene.options.json_key.clone(),
                script_classes: scene.options.script_classes.clone(),
                skip_name_suffixes: scene.options.skip_name_suffixes.clone(),
            }),
        }
    });
    // A real missing source, independent of the successful inputs' views.
    for (i, (id, _, _)) in input.iter().chain(bundles(false).iter()).enumerate() {
        if *id == "missing" {
            fs::remove_file(h.source(i)).unwrap();
        }
    }
    let mut c = h.controller(Arc::new(NoHooks));
    assert_eq!(
        c.drive_local(h.execution.as_mut().unwrap()),
        Boundary::ReadyForWinners
    );
    let result = inputs
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let mut records = h.records(i);
            let mut files = BTreeMap::new();
            for record in &mut records {
                if let Some(file) = record["file"].as_str() {
                    files.insert(
                        file.to_owned(),
                        fs::read(h.scratch.join(format!("done/{i}/{file}"))).unwrap(),
                    );
                }
                if record["type"] == "input_summary" {
                    let row = record.as_object_mut().unwrap();
                    // Only wall time, meter peaks and filesystem block accounting
                    // are excluded. Preserve record order, work and evictions.
                    for field in [
                        "ms",
                        "input_peak",
                        "dependency_peak",
                        "decode_work_peak",
                        "claimed_bytes",
                    ] {
                        row.remove(field);
                    }
                }
            }
            ((*id).to_owned(), (records, files))
        })
        .collect();
    h.execution.as_mut().unwrap().quiesce().unwrap();
    result
}
fn solo(scene: &AtlasScene, input: &str) -> AtlasScene {
    let mut seen = BTreeSet::new();
    let mut stack = vec![input];
    while let Some(id) = stack.pop() {
        if seen.insert(id) {
            stack.extend(
                scene
                    .bundles
                    .iter()
                    .find(|b| b.id == id)
                    .unwrap()
                    .deps
                    .iter()
                    .map(String::as_str),
            );
        }
    }
    AtlasScene {
        bundles: scene
            .bundles
            .iter()
            .filter(|b| seen.contains(b.id.as_str()))
            .cloned()
            .collect(),
        inputs: vec![input.to_owned()],
        options: scene.options.clone(),
    }
}

#[test]
fn an_inputs_records_do_not_depend_on_what_runs_beside_it() {
    let () = guard::finishes("determinism matrix", || {
        let mut scenes: Vec<_> = unityrs_testkit::ngui::scenes()
            .unwrap()
            .into_iter()
            .filter(|(name, _)| {
                [
                    "struct_maintex_in_dependency",
                    "json_bytes_with_textures",
                    "streamed_sheets",
                    "repeated_rows",
                    "webfile_sibling",
                    "textured_winners",
                ]
                .contains(name)
            })
            .collect();
        // Explicit same-named CAB/winner scene, plus staging and peek failures
        // outside successful views and two holders of the same failed member.

        for (_, scene) in &mut scenes {
            let mut same_cab = scene.bundles[0].clone();
            same_cab.id = "same-cab-outside".to_owned();
            let nested = SceneBundle {
                id: "nested-other".to_owned(),
                data: unityrs_testkit::env::bundle(&[("nested", same_cab.data.clone())]),
                deps: same_cab.deps.clone(),
            };
            scene.bundles.extend([same_cab, nested]);
            scene
                .inputs
                .extend(["same-cab-outside", "nested-other"].map(str::to_owned));
            scene.bundles.extend([
                SceneBundle {
                    id: "missing".into(),
                    data: clear(1),
                    deps: vec![],
                },
                SceneBundle {
                    id: "bad".into(),
                    data: b"bad".to_vec(),
                    deps: vec![],
                },
                SceneBundle {
                    id: "holder".into(),
                    data: clear(1),
                    deps: vec!["bad".into()],
                },
                SceneBundle {
                    id: "stage-first".into(),
                    data: clear(1),
                    deps: vec!["bad".into(), "missing".into()],
                },
            ]);
            scene
                .inputs
                .extend(["missing", "bad", "holder", "stage-first"].map(str::to_owned));
            let expected: BTreeMap<_, _> = scene
                .inputs
                .iter()
                .flat_map(|id| run(&solo(scene, id), &[id], 1, 1))
                .collect();
            assert!(
                expected.values().any(|(_, files)| !files.is_empty()),
                "scene must compare candidate bytes"
            );
            for threads in [1, 2, 8] {
                for slots in [1, 4] {
                    for seed in 0..20 {
                        let mut inputs: Vec<_> = scene.inputs.iter().map(String::as_str).collect();
                        let mut rng = Rng::new(seed);
                        for i in (1..inputs.len()).rev() {
                            inputs.swap(i, rng.below(i + 1));
                        }
                        assert_eq!(
                            run(scene, &inputs, threads, slots),
                            expected,
                            "threads={threads}, slots={slots}, seed={seed}"
                        );
                    }
                }
            }
        }
    });
}
