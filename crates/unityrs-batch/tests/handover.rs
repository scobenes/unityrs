//! The library run's hand-over at every seam: one stop, requested at the
//! seam's first occurrence, ends in one finisher whose head and rows follow
//! `BATCH_SPEC.md` 7.4 by the phase reached; no parked owner, runtime or
//! Execution is dropped before the finisher returns, and no `done/` entry
//! appears after the hand-over.
mod common;
mod publication_support;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    controller::{self, Event, Point},
    finish::{Finished, HandOver, Selected},
    lifetime::Stop,
    phase1,
    progress::{Phase, Snapshot, live::Service},
    publish,
    reason::{Code, StopReason},
    run::{self, Lifetime, Observer, Options, Seam},
};
use unityrs_testkit::{ngui, scratch::Scratch};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

/// Where the stop is requested: the first time this seam is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    StageDispatched,
    StageReceived,
    StageBarrier,
    PeekBarrier,
    PeekDispatched,
    PeekReceived,
    AttemptDispatched,
    AttemptReceived,
    BeforeCommitLoad,
    AfterRenameTrace,
    Selection,
    Outcomes,
    Transfer,
    BeforeCommit,
    Authorized,
    Outcome,
    Drained,
    Verified,
    /// After a normal hand-over, before the finisher's take.
    HandedOver,
}
const SEAMS: [At; 18] = [
    At::StageDispatched,
    At::StageReceived,
    At::StageBarrier,
    At::PeekBarrier,
    At::PeekDispatched,
    At::PeekReceived,
    At::AttemptDispatched,
    At::AttemptReceived,
    At::BeforeCommitLoad,
    At::AfterRenameTrace,
    At::Selection,
    At::Outcomes,
    At::Transfer,
    At::BeforeCommit,
    At::Authorized,
    At::Outcome,
    At::Drained,
    At::Verified,
];

struct Probe {
    at: At,
    stop: Stop,
    lifetime: Arc<Lifetime>,
    fired: AtomicBool,
    failures: AtomicUsize,
    drops: Arc<AtomicUsize>,
    done: PathBuf,
    seen: Mutex<Seen>,
}
#[derive(Default, Debug)]
struct Seen {
    phase: Option<Phase>,
    running: bool,
    selected: Option<&'static str>,
    pending: Option<u32>,
    done_at_handover: Vec<String>,
    drops: Vec<usize>,
    taken: Option<Stop>,
    end: Option<unityrs_batch::finish::ResultEnd>,
}
impl Probe {
    fn hit(&self, at: At) {
        if at == self.at && !self.fired.swap(true, Ordering::SeqCst) {
            self.lifetime.request(self.stop);
        }
    }
    fn drops(&self) {
        self.seen
            .lock()
            .unwrap()
            .drops
            .push(self.drops.load(Ordering::SeqCst));
    }
}
fn listing(done: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(done)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}
impl unityrs_export::ngui::Hooks for Probe {}
impl controller::Hooks for Probe {
    fn event(&self, event: Event) {
        match event {
            Event::StageDispatched(_) => self.hit(At::StageDispatched),
            Event::StageReceived(_) => self.hit(At::StageReceived),
            Event::PeekDispatched(_) => self.hit(At::PeekDispatched),
            Event::PeekReceived(_) => self.hit(At::PeekReceived),
            Event::AttemptDispatched(_) => self.hit(At::AttemptDispatched),
            Event::AttemptReceived(_) => self.hit(At::AttemptReceived),
            _ => {}
        }
    }
    fn point(&self, point: Point) {
        match point {
            Point::BeforeCommitLoad(_) => self.hit(At::BeforeCommitLoad),
            Point::AfterRenameTrace(_) => self.hit(At::AfterRenameTrace),
            Point::FailureCreated(_) => {
                // The missing input's record is the staging barrier's; the
                // unreadable one's the peek barrier's.
                let n = self.failures.fetch_add(1, Ordering::SeqCst);
                self.hit(if n == 0 {
                    At::StageBarrier
                } else {
                    At::PeekBarrier
                });
            }
            _ => {}
        }
    }
}
impl publish::Hooks for Probe {
    fn point(&self, point: publish::Point) {
        match point {
            publish::Point::Transfer => self.hit(At::Transfer),
            publish::Point::BeforeCommit => self.hit(At::BeforeCommit),
            publish::Point::Authorized => self.hit(At::Authorized),
            publish::Point::Outcome => self.hit(At::Outcome),
            publish::Point::Drained => self.hit(At::Drained),
            _ => {}
        }
    }
}
impl Observer for Probe {
    fn seam(&self, seam: Seam) {
        self.hit(match seam {
            Seam::Selection => At::Selection,
            Seam::Outcomes => At::Outcomes,
            Seam::Verified => At::Verified,
        });
    }
    fn handed_over(&self, h: &HandOver) {
        self.hit(At::HandedOver);
        {
            let mut seen = self.seen.lock().unwrap();
            seen.phase = Some(h.phase);
            seen.running = h.running;
            seen.pending = h.pending;
            seen.selected = h.selected.as_ref().map(|s| match s {
                Selected::Owned(_) => "owned",
                Selected::Parts { .. } => "parts",
            });
            seen.done_at_handover = listing(&self.done);
        }
        self.drops();
    }
    fn taken(&self, stop: Option<Stop>) {
        self.seen.lock().unwrap().taken = stop;
        self.drops();
    }
    fn finished(&self, f: &Finished) {
        self.seen.lock().unwrap().end = Some(f.end);
        self.drops();
    }
}

/// Inputs: `a` and `b` each export one texture; `missing` fails staging;
/// `junk` stages but is not a bundle.
fn scene(scratch: &Scratch) -> Value {
    for name in ["spool", "sink"] {
        fs::create_dir(scratch.join(name)).unwrap();
    }
    for (name, id) in [("a", 1), ("b", 2)] {
        let file = ngui::file(
            &[],
            &[ngui::texture(
                id,
                name.as_bytes(),
                4,
                (1, 1),
                vec![1, 2, 3, 255],
            )],
            true,
        )
        .unwrap();
        fs::write(
            scratch.join(name),
            unityrs_testkit::env::bundle(&[(&format!("CAB-{name}"), file)]),
        )
        .unwrap();
    }
    fs::write(scratch.join("junk"), b"not a bundle").unwrap();
    let mut spec = common::base();
    spec["threads"] = 1.into();
    spec["download_concurrency"] = 1.into();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["inputs"] = json!([
        {"id":"a","path":scratch.join("a")},
        {"id":"b","path":scratch.join("b")},
        {"id":"missing","path":scratch.join("absent")},
        {"id":"junk","path":scratch.join("junk")},
    ]);
    spec["dependencies"] = json!([]);
    spec["outputs"] = json!({"textures":{"key":"{input}/{name}"}});
    spec
}
struct Outcome {
    result: Result<unityrs_batch::finish::ResultEnd, run::Error>,
    seen: Seen,
    drops: usize,
    done_after: Vec<String>,
    head: Option<Value>,
    rows: std::collections::BTreeMap<&'static str, Option<Vec<Value>>>,
}
const FAMILIES: [&str; 8] = [
    "bundles",
    "inputs",
    "outputs",
    "skips",
    "diagnostics",
    "collisions",
    "archives",
    "archive_members",
];
fn execute(at: At) -> Outcome {
    execute_with(at, Stop::external(StopReason::Sigterm))
}
fn execute_with(at: At, stop: Stop) -> Outcome {
    let scratch = Scratch::new("handover");
    let spec = scene(&scratch);
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let lifetime = Arc::new(Lifetime::default());
    let drops = Arc::new(AtomicUsize::new(0));
    let probe = Arc::new(Probe {
        at,
        stop,
        lifetime: lifetime.clone(),
        fired: AtomicBool::new(false),
        failures: AtomicUsize::new(0),
        drops: drops.clone(),
        done: scratch.join("spool/runs/r1/done"),
        seen: Mutex::default(),
    });
    let result = run::drive_with(
        prepared,
        0,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &Options {
            lifetime,
            controller_hooks: probe.clone(),
            publish_hooks: probe.clone(),
            observer: probe.clone(),
            parked_drops: Some(drops.clone()),
            spool: None,
        },
    );
    assert!(probe.fired.load(Ordering::SeqCst), "{at:?} was not reached");
    let head_path = scratch.join("sink/_unityrs/r1/manifest.json");
    let head: Option<Value> = fs::read(&head_path)
        .ok()
        .map(|b| serde_json::from_slice(&b).unwrap());
    let mut rows = std::collections::BTreeMap::new();
    if let Some(head) = &head {
        for family in FAMILIES {
            let path = scratch.join(format!(
                "sink/_unityrs/r1/{}/{family}.0.ndjson",
                head["attempt"].as_u64().unwrap()
            ));
            rows.insert(
                family,
                fs::read_to_string(path).ok().map(|text| {
                    text.lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect()
                }),
            );
        }
    }
    let seen = std::mem::take(&mut *probe.seen.lock().unwrap());
    Outcome {
        result,
        seen,
        drops: drops.load(Ordering::SeqCst),
        done_after: listing(&scratch.join("spool/runs/r1/done")),
        head,
        rows,
    }
}

fn statuses(rows: &[Value], name: &str) -> Vec<(String, Option<String>)> {
    rows.iter()
        .map(|row| {
            (
                row[name].as_str().unwrap().to_owned(),
                row["reason"].as_str().map(str::to_owned),
            )
        })
        .collect()
}
fn count(rows: &[Value], status: &str) -> u64 {
    rows.iter().filter(|row| row["status"] == status).count() as u64
}
type Rows = &'static [(&'static str, Option<&'static str>)];
struct Expected {
    phase: Phase,
    done: &'static [&'static str],
    selected: Option<&'static str>,
    bundles: Rows,
    inputs: Rows,
    outputs: Option<Rows>,
}
const NOT_STAGED: Rows = &[
    ("not_staged", None),
    ("not_staged", None),
    ("not_staged", None),
    ("not_staged", None),
];
const STAGED: Rows = &[
    ("staged", None),
    ("staged", None),
    ("staged", None),
    ("failed", Some("source_missing")),
];
const QUEUED: Rows = &[
    ("not_run", None),
    ("not_run", None),
    ("not_run", None),
    ("not_run", None),
];
const MISSING_STOPPED: Rows = &[
    ("not_run", None),
    ("not_run", None),
    ("not_run", None),
    ("not_run", Some("stopped")),
];
const MISSING_FAILED: Rows = &[
    ("not_run", None),
    ("not_run", None),
    ("not_run", None),
    ("failed", Some("source_missing")),
];
const ATTEMPTING: Rows = &[
    ("not_run", Some("stopped")),
    ("not_run", None),
    ("failed", Some("not_a_bundle")),
    ("failed", Some("source_missing")),
];
const RECEIVED: Rows = &[
    ("complete", None),
    ("complete", None),
    ("failed", Some("not_a_bundle")),
    ("failed", Some("source_missing")),
];
const ALL: &[&str] = &["0", "1", "2", "3"];
fn expected(at: At) -> Expected {
    let early = |phase, done, bundles, inputs| Expected {
        phase,
        done,
        selected: None,
        bundles,
        inputs,
        outputs: None,
    };
    let late = |selected, outputs| Expected {
        phase: Phase::Publish,
        done: ALL,
        selected: Some(selected),
        bundles: STAGED,
        inputs: RECEIVED,
        outputs: Some(outputs),
    };
    let none: Rows = &[("not_written", None), ("not_written", None)];
    let first: Rows = &[("written", None), ("not_written", None)];
    let both: Rows = &[("written", None), ("written", None)];
    match at {
        At::StageDispatched => early(Phase::Stage, &[], NOT_STAGED, QUEUED),
        At::StageReceived => early(
            Phase::Stage,
            &[],
            &[
                ("staged", None),
                ("not_staged", None),
                ("not_staged", None),
                ("not_staged", None),
            ],
            QUEUED,
        ),
        // The missing input's failure record is made, not committed: its
        // uncommitted bit makes it `stopped`.
        At::StageBarrier | At::BeforeCommitLoad => {
            early(Phase::Stage, &[], STAGED, MISSING_STOPPED)
        }
        At::PeekBarrier => early(
            Phase::Extract,
            &["2"],
            STAGED,
            &[
                ("not_run", None),
                ("not_run", None),
                ("not_run", Some("stopped")),
                ("failed", Some("source_missing")),
            ],
        ),
        // The rename landed before the trace; the commit completes.
        At::PeekDispatched | At::PeekReceived | At::AfterRenameTrace => {
            early(Phase::Extract, &["2"], STAGED, MISSING_FAILED)
        }
        At::AttemptDispatched | At::AttemptReceived => {
            early(Phase::Extract, &["2", "3"], STAGED, ATTEMPTING)
        }
        At::Selection => early(Phase::Winners, ALL, STAGED, RECEIVED),
        // Phase 7 reached with no outcome files: every status reads
        // `not_written` from the phase-6 artifacts alone.
        At::Outcomes => late("parts", none),
        At::Transfer | At::BeforeCommit => late("owned", none),
        // Authorized: the rename and its outcome are one gate.
        At::Authorized | At::Outcome => late("owned", first),
        At::Drained | At::Verified | At::HandedOver => late("owned", both),
    }
}
fn pairs(rows: Rows) -> Vec<(String, Option<String>)> {
    rows.iter()
        .map(|(s, r)| ((*s).to_owned(), r.map(str::to_owned)))
        .collect()
}

#[test]
fn every_seam_hands_over_once_with_the_rows_of_the_phase_reached() {
    for at in SEAMS {
        let o = execute(at);
        let want = expected(at);
        let end = o.result.unwrap();
        assert_eq!(
            (end.code, end.status, end.reason),
            (Code::Stopped, Some("incomplete"), Some("sigterm")),
            "{at:?}"
        );
        assert_eq!(o.seen.end, Some(end), "{at:?}");
        assert_eq!(o.seen.taken, Some(Stop::external(StopReason::Sigterm)));
        assert!(o.seen.running, "{at:?}");
        assert_eq!(o.seen.phase, Some(want.phase), "{at:?}");
        assert_eq!(o.seen.selected, want.selected, "{at:?}");
        assert_eq!(o.seen.pending, None, "{at:?}");
        // No parked owner (the Execution and its runtime among them) drops
        // before the finisher returns; all of them once it has.
        assert_eq!(o.seen.drops, [0, 0, 0], "{at:?}");
        assert_eq!(o.drops, 1, "{at:?}");
        // Nothing commits after the hand-over.
        assert_eq!(o.seen.done_at_handover, want.done, "{at:?}");
        assert_eq!(o.done_after, want.done, "{at:?}");

        let head = o.head.as_ref().unwrap();
        assert_eq!(head["status"], "incomplete", "{at:?}");
        assert_eq!(head["exit_code"], 6);
        assert_eq!(head["stop_reason"], "sigterm");
        assert_eq!(head["phase"], want.phase.name(), "{at:?}");
        let publish = want.phase == Phase::Publish;
        for family in ["outputs", "collisions", "archives", "archive_members"] {
            assert_eq!(head["rows"][family].is_null(), !publish, "{at:?} {family}");
        }
        for family in ["bundles", "inputs", "skips", "diagnostics"] {
            assert!(!head["rows"][family].is_null(), "{at:?} {family}");
        }
        let bundles = o.rows["bundles"].as_ref().unwrap();
        let inputs = o.rows["inputs"].as_ref().unwrap();
        assert_eq!(statuses(bundles, "status"), pairs(want.bundles), "{at:?}");
        assert_eq!(statuses(inputs, "status"), pairs(want.inputs), "{at:?}");
        // A dispatched `stopped` row times dispatch to the hand-over; a
        // barrier's failure record was never dispatched, so has no `ms`.
        let attempting = matches!(at, At::AttemptDispatched | At::AttemptReceived);
        for row in inputs {
            let timed = row["status"] == "complete" || row["reason"] == "stopped" && attempting;
            assert_eq!(!row["ms"].is_null(), timed, "{at:?} {row}");
        }
        let counts = &head["counts"];
        assert_eq!(counts["inputs"], 4);
        assert_eq!(counts["inputs_complete"], count(inputs, "complete"));
        assert_eq!(counts["inputs_failed"], count(inputs, "failed"));
        assert_eq!(counts["inputs_not_run"], count(inputs, "not_run"));
        assert_eq!(counts["bundles_failed"], count(bundles, "failed"));
        match want.outputs {
            None => assert_eq!(o.rows["outputs"], None, "{at:?}"),
            Some(rows) => {
                let outputs = o.rows["outputs"].as_ref().unwrap();
                assert_eq!(statuses(outputs, "status"), pairs(rows), "{at:?}");
                assert_eq!(counts["outputs_written"], count(outputs, "written"));
                assert_eq!(counts["outputs_not_written"], count(outputs, "not_written"));
            }
        }
    }
}

/// A stop after the controller's last check changes nothing: the finisher
/// takes it, and writes and returns the normal end (9.1).
#[test]
fn a_late_stop_after_a_normal_hand_over_keeps_the_normal_head() {
    let o = execute(At::HandedOver);
    assert_eq!(o.seen.taken, Some(Stop::external(StopReason::Sigterm)));
    let end = o.result.unwrap();
    // Two failed inputs: a 4, named by neither the stop nor its reason.
    assert_eq!(
        (end.code, end.status, end.detail),
        (Code::Incomplete, Some("incomplete"), None)
    );
    assert_ne!(end.reason, Some("sigterm"));
    let head = o.head.unwrap();
    assert_eq!(head["exit_code"], 4);
    assert_eq!(head["stop_reason"], Value::Null);
    assert_eq!(head["phase"], "publish");
    assert_eq!(o.seen.drops, [0, 0, 0]);
    assert_eq!(o.drops, 1);
    let want = expected(At::HandedOver);
    let outputs = o.rows["outputs"].as_ref().unwrap();
    assert_eq!(statuses(outputs, "status"), pairs(want.outputs.unwrap()));
    assert_eq!(o.done_after, want.done);
}

/// A stop that names a culprit puts its reason on the culprit's row: an
/// input's when it is uncommitted, a bundle's always; no other row's.
#[test]
fn a_culprits_row_carries_the_stops_reason_as_its_detail() {
    use unityrs_batch::control::Culprit;
    let input = Stop {
        culprit: Culprit::Input(0),
        ..Stop::external(StopReason::Internal)
    };
    let o = execute_with(At::AttemptDispatched, input);
    let end = o.result.unwrap();
    assert_eq!((end.code, end.reason), (Code::Internal, Some("internal")));
    let inputs = o.rows["inputs"].as_ref().unwrap();
    assert_eq!(inputs[0]["reason"], "stopped");
    assert_eq!(inputs[0]["detail"], "internal");
    assert!(inputs[1..].iter().all(|row| row["detail"].is_null()));
    assert_eq!(o.head.unwrap()["stop_reason"], "internal");

    let bundle = Stop {
        culprit: Culprit::Bundle(1),
        ..Stop::external(StopReason::HostIo)
    };
    let o = execute_with(At::StageReceived, bundle);
    assert_eq!(o.result.unwrap().code, Code::Host);
    let bundles = o.rows["bundles"].as_ref().unwrap();
    assert_eq!(bundles[1]["detail"], "host_io");
    assert_eq!(bundles[1]["status"], "not_staged");
    assert!(
        bundles
            .iter()
            .enumerate()
            .all(|(i, row)| i == 1 || row["detail"].is_null())
    );
    // An input whose own bundle was the culprit is not itself named.
    let inputs = o.rows["inputs"].as_ref().unwrap();
    assert!(inputs.iter().all(|row| row["detail"].is_null()));
}
