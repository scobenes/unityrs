//! Unit collision rules, representative flags and publication withholding.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/member_scene.rs"]
mod member_scene;
use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_batch::{
    phase::Account,
    sort::{NoHooks, Options},
    spool::{
        Counter, Host,
        workspace::{Sealed, Workspace},
    },
    winners::{self, Context, Prepared},
};
use unityrs_testkit::scratch::Scratch;
fn rows(workspace: &Workspace, file: Option<&Sealed>) -> Vec<Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Some(file) = file {
        workspace
            .open(file, &|| false)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
    }
    bytes
        .split(|&b| b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}
#[test]
#[allow(clippy::too_many_lines)] // One fixture compares ordered collision rows and withholding overlays.
fn unit_collision_rows_keep_rules_and_exactly_one_representative_under_withholding() {
    for blocked in [false, true] {
        let scratch = Scratch::new("member-rows");
        let case = member_scene::scene(blocked);
        let spec = case.spec().spec;
        let account = Account::new(64 << 20);
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
        let handoff = winners::complete(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options {
                run_records: 2,
                fan_in: 2,
                ..Options::default()
            },
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        })
        .unwrap();
        assert_eq!(handoff.publication_blocked, blocked);
        let collisions = rows(&workspace, handoff.collisions.as_ref());
        let actual: Vec<_> = collisions
            .iter()
            .map(|r| {
                json!([
                    r["key"],
                    r["input"],
                    r["rule"],
                    r["winner"],
                    r["flip"],
                    r["pixels_differ"]
                ])
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                json!(["a", "a", "first_input", true, true, false]),
                json!(["a", "b", "first_input", false, true, false]),
                json!(["chosen", "a", "prefer_input", false, false, false]),
                json!(["chosen", "b", "prefer_input", true, false, false]),
                json!(["held", "a", "withheld", true, true, false]),
                json!(["held", "b", "withheld", false, true, false]),
                json!(["outside", "a", "unit_conflict", true, true, false]),
                json!(["outside", "b", "first_input", false, true, false]),
                json!(["p", "a", "unit_conflict", true, true, false]),
                json!(["p", "b", "first_input", false, true, false]),
                json!(["shared", "a", "first_input", true, false, false]),
                json!(["shared", "b", "unit_conflict", false, false, false]),
                json!(["shared", "c", "unit", false, false, false]),
            ]
        );
        let outputs = rows(&workspace, handoff.outputs.as_ref());
        let get = |key| outputs.iter().find(|r| r["key"] == key).unwrap();
        for (key, status, reason) in [
            ("held", "withheld", "withheld"),
            ("outside", "withheld", "unit_conflict"),
            ("p", "unwritable_key", "unwritable_key(prefix_conflict)"),
            ("z", "withheld", "unit_conflict"),
        ] {
            assert_eq!(get(key)["status"], status);
            assert_eq!(get(key)["reason"], reason);
        }
        assert_eq!(get("chosen")["input"], "b");
        assert_eq!(
            get("chosen")["status"],
            if blocked { "withheld" } else { "not_written" }
        );
        assert_eq!(
            get("chosen")["reason"],
            if blocked {
                json!("publish_incomplete")
            } else {
                Value::Null
            }
        );
        let members = rows(&workspace, handoff.archive_members.as_ref());
        assert_eq!(members.len(), 6);
        assert!(
            members
                .iter()
                .all(|m| m["path"] != "loser" && m["path"] != "withheld")
        );
        assert!(members.iter().any(|m| m["key"] == "chosen"
            && m["input"] == "b"
            && m["reason"] == "prefix_conflict"));
        assert!(
            members
                .iter()
                .any(|m| m["key"] == "other" && m["reason"] == "duplicate_member")
        );
        let archives = rows(&workspace, handoff.archives.as_ref());
        assert_eq!(archives[0]["status"], "empty");
        assert_eq!(archives[0]["left_out"], 1);
        assert_eq!(archives[2]["members"], 2);
        assert_eq!(
            archives[2]["status"],
            if blocked { "withheld" } else { "not_written" }
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn visible_hash_comparison_requires_both_hashes_and_never_marks_the_representative() {
    let scratch = Scratch::new("collision-pixels");
    let case = harness::Case::new(vec![
        harness::candidate("a", "key", None, 1),
        harness::candidate("b", "key", None, 2),
        harness::candidate("c", "key", None, 3),
    ]);
    let spec = case.spec().spec;
    let account = Account::new(64 << 20);
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
    for (i, path) in commits.paths.iter().enumerate().take(2) {
        let original = std::fs::read(path).unwrap();
        let end = original.iter().position(|&b| b == b'\n').unwrap() + 1;
        let mut row: Value = serde_json::from_slice(&original[..end]).unwrap();
        row["kind"] = "texture".into();
        row["mode"] = "RGBA".into();
        row["visible_hash"] = format!("{i:02x}").repeat(32).into();
        let mut changed = serde_json::to_vec(&row).unwrap();
        changed.push(b'\n');
        changed.extend_from_slice(&original[end..]);
        std::fs::write(path, changed).unwrap();
    }
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let handoff = winners::complete(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    })
    .unwrap();
    let collisions = rows(&workspace, handoff.collisions.as_ref());
    assert_eq!(
        collisions
            .iter()
            .map(|r| (&r["input"], &r["winner"], &r["pixels_differ"]))
            .collect::<Vec<_>>(),
        vec![
            (&json!("a"), &json!(true), &json!(false)),
            (&json!("b"), &json!(false), &json!(true)),
            (&json!("c"), &json!(false), &json!(false))
        ]
    );
    assert_eq!(account.snapshot().input.used, 0);
}
