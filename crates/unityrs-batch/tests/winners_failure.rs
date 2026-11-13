//! A real partially flushed B4 attempt is never offered as a committed source.
mod artifact_support;
mod attempt_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
use std::sync::Arc;
use unityrs_batch::{
    attempt,
    phase::Account,
    records::{self, Status, Summary},
    sort::{NoHooks, Options},
    spool::{Directory, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
};
use unityrs_testkit::ngui::texture;

#[test]
fn failed_attempt_after_records_flush_competes_only_through_separate_failure_commit() {
    let objects = (0..140)
        .map(|i| {
            texture(
                i,
                format!("{i:04}/{}", vec!["a".repeat(200); 4].join("/")).as_bytes(),
                4,
                (1, 1),
                vec![0; 4],
            )
        })
        .collect::<Vec<_>>();
    let h = attempt_support::Harness::new(&attempt_support::simple(&objects), "x");
    let mut limits = attempt_support::limits();
    limits.max_export_work = 450_000;
    let end = h.run(limits, &unityrs_export::ngui::NoHooks);
    assert_eq!(end.state, attempt::State::Failed);
    let path = h.directory.path().join("records.ndjson");
    let abandoned = std::fs::read(&path).unwrap();
    assert!(abandoned.len() >= records::BUFFER);
    assert!(!String::from_utf8_lossy(&abandoned).contains("input_summary"));
    assert!(h.directory.path().join("o0").exists());
    let original_claim = h.directory.claimed();
    let account = Account::new(64 << 20);
    let mut failure =
        Directory::create(h.scratch.join("fail/0"), h.counter.clone(), Arc::new(Host)).unwrap();
    records::write_failure_record(
        &failure,
        account.meter(),
        Summary {
            input: "x",
            status: Status::Failed,
            reason: Some(records::Reason::Text("limit(export_work, 450000)".into())),
            detail: None,
            figures: Some(end.figures),
            spooled_part: end.spooled_part,
            claimed_bytes: 0,
            cut: false,
        },
    )
    .unwrap();
    failure.rename(h.scratch.join("committed-failure")).unwrap();
    let mut case = harness::Case::new(Vec::new());
    case.failed.insert("x".into());
    let spec = case.spec().spec;
    let prepared = Prepared::from_spec(&spec).unwrap();
    let commits = harness::Commits {
        paths: vec![failure.path().join("records.ndjson")],
        failed: vec![true],
        order: vec![0],
    };
    let mut workspace =
        Workspace::create(h.scratch.join("w"), h.counter.clone(), Arc::new(Host)).unwrap();
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
    assert!(artifacts.winners.is_none());
    assert_eq!(artifacts.counts, winners::Counts::default());
    assert_eq!(std::fs::read(path).unwrap(), abandoned);
    assert_eq!(h.directory.claimed(), original_claim);
}
