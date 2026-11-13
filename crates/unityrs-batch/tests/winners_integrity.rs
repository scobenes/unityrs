mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
use harness::{Case, candidate};
use std::sync::Arc;
use unityrs_batch::{
    phase::{Account, Error},
    records::{self, Record, Status, Text},
    sort::{NoHooks, Options},
    spool::{Counter, Directory, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
    work::Work,
};
use unityrs_export::ngui::OutputKind;
use unityrs_testkit::scratch::Scratch;

struct Sizes {
    counted: u64,
}
impl winners::Observer for Sizes {
    fn sort(&self, _: u64, encoded: u64, _: u64) {
        assert!(
            encoded <= self.counted,
            "projection {encoded} > committed candidate bytes {}",
            self.counted
        );
    }
}
#[test]
fn missing_duplicate_roots_and_duplicate_key_input_are_internal() {
    for rows in [
        vec![candidate("a", "member", Some("root"), 1)],
        vec![
            candidate("a", "root", Some("root"), 1),
            candidate("a", "root", Some("root"), 2),
        ],
        vec![
            candidate("a", "plain", None, 1),
            candidate("a", "plain", None, 2),
        ],
    ] {
        let case = Case::new(rows);
        let scratch = Scratch::new("winner-corrupt");
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let account = Account::new(64 << 20);
        let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
        let result = winners::run(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options {
                run_records: 1,
                fan_in: 2,
                ..Options::default()
            },
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        });
        assert_eq!(result.unwrap_err(), Error::internal());
        assert_eq!(account.snapshot().input.used, 0);
    }
}
#[test]
fn maximum_repaired_candidate_and_all_projection_frames_fit_twice_counted_work() {
    let scratch = Scratch::new("winner-width");
    let mut case = Case::new(Vec::new());
    case.inputs.push("a".into());
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let directory =
        Directory::create(scratch.join("committed"), counter.clone(), Arc::new(Host)).unwrap();
    let mut work = Work::new(u64::MAX);
    let mut writer = records::Writer::new(&directory, account.meter()).unwrap();
    let key = format!(
        "a000/{}/{}/{}/{}",
        "x".repeat(255),
        "x".repeat(255),
        "x".repeat(255),
        "x".repeat(251)
    );
    assert_eq!(key.len(), 1024);
    let root = format!(
        "u000/{}/{}/{}/{}",
        "u".repeat(255),
        "u".repeat(255),
        "u".repeat(255),
        "u".repeat(251)
    );
    let raw = vec![0xff; 1024];
    for (ordinal, k) in [&root, &key].iter().enumerate() {
        let hashes =
            records::copy_output(&directory, u64::try_from(ordinal).unwrap(), b"x", &mut work)
                .unwrap();
        writer
            .append(
                Record::Candidate(records::Candidate {
                    key: k,
                    unit: Some(&root),
                    input: "a",
                    serialized_file: Text::Raw(&raw),
                    kind: OutputKind::AtlasJson,
                    path_id: i64::MIN,
                    hashes,
                    picture: None,
                    size: 1,
                    plan_index: u64::try_from(ordinal).unwrap(),
                    cut: true,
                }),
                &mut work,
            )
            .unwrap();
    }
    let mut summary = artifact_support::summary(Status::Complete);
    summary.input = "a";
    let _summary = writer.finish(summary, &mut work).unwrap();
    let commits = harness::Commits {
        paths: vec![directory.path().join("records.ndjson")],
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
        options: Options {
            run_records: 1,
            ..Options::default()
        },
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &Sizes {
            counted: u64::try_from(
                std::fs::read_to_string(directory.path().join("records.ndjson"))
                    .unwrap()
                    .lines()
                    .filter(|line| line.contains("\"type\":\"candidate\""))
                    .map(|line| line.len() + 1)
                    .sum::<usize>(),
            )
            .unwrap(),
        },
    };
    let artifacts = winners::run(&mut cx).unwrap();
    let mut cursor =
        winners::stream::Cursor::new(artifacts.winners.as_ref(), &winners::stream::By::Key, &cx)
            .unwrap();
    let mut count = 0;
    while let Some(row) = cursor.row().unwrap() {
        assert_eq!(row.serialized_file, "�".repeat(1024));
        assert_eq!(row.path_id, i64::MIN);
        assert!(row.cut);
        count += 1;
        cursor.advance(&|| false).unwrap();
    }
    assert_eq!(count, 2);
    drop(cursor);
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn failure_commit_competes_for_nothing_even_beside_abandoned_candidate_artifacts() {
    let scratch = Scratch::new("winner-failure");
    let mut case = Case::new(vec![candidate("a", "k", None, 1)]);
    case.failed.insert("z".into());
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
    // This valid candidate artifact stands for the attempt abandoned after a
    // copy-out failure. Only the controller's separate failure path is offered.
    let mut abandoned = Case::new(vec![candidate("z", "k", None, 9)]);
    abandoned.inputs.push("z".into());
    let abandoned_scratch = Scratch::new("abandoned");
    abandoned.commit(
        &abandoned_scratch,
        &counter,
        &account,
        &abandoned.spec().spec,
    );
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
    let artifacts = winners::run(&mut cx).unwrap();
    let actual = case.decisions(&cx, &artifacts);
    assert_eq!(
        actual,
        unityrs_export::ngui::decide(&case.candidates, &case.prefer, &case.failed, true)
    );
    assert_eq!(actual[0].winner, "a");
}

#[test]
fn empty_and_failure_only_inputs_with_256_archives_leave_no_b6b_spool_files() {
    for failed in [false, true] {
        let mut case = Case::new(Vec::new());
        case.inputs.push("a".into());
        if failed {
            case.failed.insert("a".into());
        }
        case.archives = (0..256).map(|i| format!("a{i:03}")).collect();
        let scratch = Scratch::new("winner-empty");
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let account = Account::new(64 << 20);
        let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
        let baseline = counter.used();
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace =
            Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
        let artifacts = winners::run(&mut Context {
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
        assert!(artifacts.keys.is_none());
        assert!(artifacts.units.is_none());
        assert!(artifacts.winners.is_none());
        assert_eq!(artifacts.counts, winners::Counts::default());
        assert_eq!(workspace.claimed(), 0);
        assert_eq!(counter.used(), baseline);
        assert!(counter.peak() - baseline <= 4096);
        assert_eq!(account.snapshot().input.used, 0);
    }
}
