//! Incrementally written committed records; no dataset or reference model is
//! retained beside the measured selection working set.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/probe.rs"]
mod probe;
use std::{
    process::Command,
    sync::{Arc, atomic::Ordering},
};
use unityrs_batch::{
    alloc::{self, Counting},
    phase::Account,
    records::{self, Record, Status},
    sort::{NoHooks, Options},
    spool::{Counter, Directory, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
    work::Work,
};
use unityrs_export::ngui::OutputKind;
use unityrs_testkit::{child, scratch::Scratch};
#[global_allocator]
static ALLOC: Counting = Counting::new();

#[test]
#[allow(clippy::too_many_lines)] // Generation, counting, and streaming verification share one child.
fn scale_child() {
    let Ok(mode) = std::env::var("UNITYRS_MEMBERS_SCALE") else {
        return;
    };
    let scratch = Scratch::new("winner-scale");
    let mut case = harness::Case::new(Vec::new());
    case.inputs.push("a".into());
    case.archives.push("archive".into());
    let spec = case.spec().spec;
    drop(case);
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let directory =
        Directory::create(scratch.join("committed"), counter.clone(), Arc::new(Host)).unwrap();
    let mut writer = records::Writer::new(&directory, account.meter()).unwrap();
    let mut work = Work::new(u64::MAX);
    let count = match mode.as_str() {
        "million" => 1_000_000,
        "unit" => 100_000,
        _ => 350_000,
    };
    let file = "s".repeat(255);
    let hashes = unityrs_batch::digest::Hashes::of(b"");
    for i in (0..count).rev() {
        let key = format!("k{i:08}");
        // Selection stress materializes records only; real payload copy-out
        // is exercised by fixture/failure tests. Empty outputs' deterministic
        // 4 KiB work floor remains counted, independently of physical length.
        work.file(0).unwrap();
        writer
            .append(
                Record::Candidate(records::Candidate {
                    key: &key,
                    unit: (mode == "unit").then_some("k00000000"),
                    input: "a",
                    serialized_file: file.as_str().into(),
                    kind: OutputKind::AtlasJson,
                    path_id: i,
                    hashes,
                    picture: None,
                    size: 0,
                    plan_index: u64::try_from(i).unwrap(),
                    cut: false,
                }),
                &mut work,
            )
            .unwrap();
        writer
            .member(
                "archive",
                records::Member {
                    archive: 0,
                    path: key.as_str().into(),
                    key: &key,
                    input: "a",
                    md5: &hashes.md5,
                    reason: None,
                    cut: false,
                },
                &mut work,
            )
            .unwrap();
    }
    let mut summary = artifact_support::summary(Status::Complete);
    summary.input = "a";
    let summary = writer.finish(summary, &mut work).unwrap();
    let record_bytes = std::fs::metadata(directory.path().join("records.ndjson"))
        .unwrap()
        .len();
    if mode != "unit" {
        assert!(record_bytes > 96 << 20);
    }
    let baseline = counter.used();
    let probe = probe::Probe::new(counter.clone(), baseline, 4096);
    let commits = harness::Commits {
        paths: vec![directory.path().join("records.ndjson")],
        failed: vec![false],
        order: vec![0],
    };
    let prepared = Prepared::from_spec(&spec).unwrap();
    // The filesystem probe samples claims before every I/O operation.
    let mut workspace = Workspace::create(
        scratch.join("w"),
        counter.clone(),
        Arc::new(probe::Fs(probe.clone())),
    )
    .unwrap();
    let mut cx = Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: if mode == "million" {
            Options {
                run_bytes: 512 * 1024,
                ..Options::default()
            }
        } else {
            Options::default()
        },
        stopped: &|| false,
        hooks: &NoHooks,
        observer: probe.as_ref(),
    };
    let (result, measured) = alloc::measured(|| winners::complete(&mut cx));
    let artifacts = result.unwrap();
    if mode == "million" {
        assert!(probe.max_runs.load(Ordering::Relaxed) > 256);
    }
    probe.sample();
    assert!(artifacts.winners.is_some());
    assert!(measured.peak <= 40 << 20, "{measured:?}");
    assert_eq!(measured.kept, 0);
    assert_eq!(account.snapshot().input.used, 0);
    assert!(account.snapshot().input.peak <= 40 << 20);
    let allowance = 6 * summary.spooled_part + 2 * 4096;
    assert!(counter.peak() - baseline <= allowance);
    for bound in &probe.bounds {
        assert!(bound.load(Ordering::Relaxed) <= allowance);
    }
    let winners = artifacts.winners.as_ref().unwrap();
    let mut cursor =
        winners::stream::Cursor::new(Some(winners), &winners::stream::By::Key, &cx).unwrap();
    let mut got = 0_i64;
    while let Some(row) = cursor.row().unwrap() {
        assert_eq!(row.path_id, got);
        got += 1;
        cursor.advance(&|| false).unwrap();
    }
    assert_eq!(got, count);
    drop(cursor);
    let mut metadata = winners::archives::Reader::open(
        cx.workspace,
        artifacts.metadata.as_ref().unwrap(),
        account.meter(),
        &|| false,
    )
    .unwrap();
    let archive = metadata.next(&|| false).unwrap().unwrap();
    assert_eq!(archive.members, u64::try_from(count).unwrap());
    let list = archive.list.unwrap();
    let mut members =
        winners::member::Reader::open(cx.workspace, &list, account.meter(), &|| false).unwrap();
    let mut got = 0;
    while members.next(&|| false).unwrap().is_some() {
        got += 1;
    }
    assert_eq!(got, count);
    drop((members, metadata));
    for (pass, (peak, bound)) in probe.peaks.iter().zip(&probe.bounds).enumerate() {
        println!(
            "MEMBERS_SCALE_PASS mode={mode} pass={pass} peak={} envelope={}",
            peak.load(Ordering::Relaxed),
            bound.load(Ordering::Relaxed)
        );
    }
    println!(
        "MEMBERS_SCALE mode={mode} candidates={count} record_bytes={record_bytes} S={} allowance={allowance} peak={} counted_peak={} account_peak={}",
        summary.spooled_part,
        counter.peak() - baseline,
        measured.peak,
        account.snapshot().input.peak
    );
}
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn counted_production_and_million_candidate_forced_run_children() {
    for mode in ["production", "million", "unit"] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "scale_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_MEMBERS_SCALE", mode),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
