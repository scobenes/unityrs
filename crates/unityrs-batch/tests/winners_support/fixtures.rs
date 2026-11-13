//! Fixture expectations are read before production selection, never recovered
//! from its committed reader. Real B4 attempts supply the competing artifacts.
use crate::attempt_support::{self, Harness};
use crate::harness::Case;
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use unityrs_batch::{
    attempt,
    phase::Account,
    sort::{NoHooks, Options},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
};
use unityrs_export::ngui::{self, Candidate, Prefer};
use unityrs_testkit::{
    ngui::{scenes, sprite_scenes},
    scratch::Scratch,
};

fn unhex(text: &str) -> [u8; 16] {
    std::array::from_fn(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
}
fn candidate(row: &J, input: &str) -> Candidate {
    Candidate {
        input: input.into(),
        key: row["key"].as_str().unwrap().into(),
        unit: row["unit"].as_str().map(str::to_owned),
        md5: unhex(
            row["md5"]
                .as_str()
                .or_else(|| row["png_md5"].as_str())
                .unwrap(),
        ),
    }
}
fn value(decisions: &[ngui::Decision], overlay: bool, failed: bool) -> J {
    decisions.iter().map(|d|json!({"key":d.key,"winner":d.winner,"flip":d.flip,
        "status":if d.publish && !(overlay && failed) {"publish"} else {"withheld"},
        "reason":if d.publish && overlay && failed {Some("publish_incomplete")} else {d.reason},
        "candidates":d.candidates.iter().map(|c|json!([c.input,c.unit,c.rule.name(),c.winner])).collect::<Vec<_>>()
    })).collect()
}
fn additional_selection(expected: &J, harness: &Harness) {
    let id = expected["input"].as_str().unwrap();
    let mut case = Case::new(
        expected["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| candidate(row, id))
            .collect(),
    );
    if case.inputs.is_empty() {
        case.inputs.push(id.into());
    }
    let spec = case.spec().spec;
    let scratch = Scratch::new("winner-nested");
    let account = Account::new(64 << 20);
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let commits = crate::harness::Commits {
        paths: vec![harness.directory.path().join("records.ndjson")],
        failed: vec![false],
        order: vec![0],
    };
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let mut cx = Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    };
    let artifacts = winners::steps123(&mut cx).unwrap();
    assert_eq!(
        case.decisions(&cx, &artifacts),
        ngui::decide(&case.candidates, &case.prefer, &case.failed, true)
    );
}
#[allow(clippy::too_many_lines)] // One independently recorded fixture family.
pub fn replay(name: &str, expected_count: usize) {
    let fixture =
        attempt_support::fixture(&format!("../unityrs-testkit/tests/fixtures/{name}.json"));
    assert_eq!(fixture["winners"].as_array().unwrap().len(), expected_count);
    let scenes = if name == "ngui" {
        scenes().unwrap()
    } else {
        sprite_scenes().unwrap()
    };
    let mut held = BTreeMap::new();
    for expected in fixture["results"].as_array().unwrap().iter().filter(|r| {
        matches!(
            r["scene"].as_str().unwrap(),
            "winners" | "sprite_winners" | "textured_winners" | "nested_cabs" | "repeated_rows"
        )
    }) {
        let scene_name = expected["scene"].as_str().unwrap();
        let id = expected["input"].as_str().unwrap();
        let scene = &scenes.iter().find(|(n, _)| *n == scene_name).unwrap().1;
        let mut harness = Harness::new(scene, id);
        if name == "sprites" {
            harness.options.atlases = false;
        }
        let result = harness.run(attempt_support::limits(), &ngui::NoHooks);
        assert_eq!(
            result.state,
            attempt::State::Done,
            "{name}/{scene_name}/{id}: {result:?}"
        );
        let actual = harness
            .rows()
            .into_iter()
            .filter(|r| r["type"] == "candidate")
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), expected["outputs"].as_array().unwrap().len());
        for (actual, expected) in actual.iter().zip(expected["outputs"].as_array().unwrap()) {
            assert_eq!(candidate(actual, id), candidate(expected, id));
        }
        harness
            .directory
            .rename(harness.scratch.join("committed"))
            .unwrap();
        if matches!(scene_name, "nested_cabs" | "repeated_rows") {
            additional_selection(expected, &harness);
        }
        held.insert((scene_name.to_owned(), id.to_owned()), harness);
    }
    for fixture_case in fixture["winners"].as_array().unwrap() {
        let ids = fixture_case["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mut case = Case::new(Vec::new());
        case.inputs.clone_from(&ids);
        case.failed = fixture_case["failed"]
            .as_array()
            .map_or_else(BTreeSet::new, |a| {
                a.iter().map(|v| v.as_str().unwrap().to_owned()).collect()
            });
        case.publish_incomplete = fixture_case["publish_incomplete"].as_bool().unwrap_or(true);
        case.prefer = fixture_case["prefer"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, p)| {
                (
                    key.clone(),
                    Prefer {
                        input: p["input"].as_str().map(str::to_owned),
                        md5: p["md5"].as_str().map(unhex),
                    },
                )
            })
            .collect();
        for expected in fixture["results"].as_array().unwrap() {
            if ids
                .iter()
                .any(|id| Some(id.as_str()) == expected["input"].as_str())
            {
                for row in expected["outputs"].as_array().unwrap() {
                    case.candidates
                        .push(candidate(row, expected["input"].as_str().unwrap()));
                }
            }
        }
        let scratch = Scratch::new("winner-fixture");
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let account = Account::new(64 << 20);
        let (mut commits, _) = case.commit(&scratch, &counter, &account, &spec);
        for i in 0..spec.input_count() {
            let id = spec.bundle(i).unwrap().id();
            if !case.failed.contains(id) {
                let scene = if name == "sprites" {
                    "sprite_winners"
                } else if id.starts_with(['p', 'q', 'r']) {
                    "textured_winners"
                } else {
                    "winners"
                };
                commits.paths[i] = held[&(scene.into(), id.into())]
                    .directory
                    .path()
                    .join("records.ndjson");
            }
        }
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
        let mut cx = Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        };
        let artifacts = winners::steps123(&mut cx).unwrap();
        let actual = case.decisions(&cx, &artifacts);
        assert_eq!(
            actual,
            ngui::decide(&case.candidates, &case.prefer, &case.failed, true)
        );
        let published = value(
            &actual,
            artifacts.publication_state(winners::State::Publish) == winners::State::Incomplete,
            !case.failed.is_empty(),
        );
        assert_eq!(
            published,
            value(
                &ngui::decide(
                    &case.candidates,
                    &case.prefer,
                    &case.failed,
                    case.publish_incomplete
                ),
                false,
                false
            )
        );
        assert_eq!(
            published, fixture_case["decisions"],
            "{}",
            fixture_case["case"]
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
}
