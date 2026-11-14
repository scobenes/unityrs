//! The stopped finisher's decisions that no seam alone shows: the one stat
//! that reconciles a pending commit, the exit code's precedence, a stop
//! racing startup, and the ends that leave `running` rather than no head.
mod common;
mod publication_support;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use unityrs_batch::{
    finish::{self, FinishCause, Finished, HandOver, Reconcile, Terminal},
    lifetime::{SpoolDetail, Stop},
    phase::Account,
    phase1,
    progress::{Phase, Snapshot, live::Service},
    reason::{Code, StopReason},
    run::{self, Lifetime, Observer, Options, Seam},
    spool::{FileSystem, Host},
};
use unityrs_testkit::{ngui, scratch::Scratch};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> io::Result<u64> {
        Ok(u64::MAX)
    }
}
/// The commit's rename fails `EIO`, after renaming (`perform`) or not; the
/// finisher's stat of `done/<input>` answers for real, or fails `stat`.
struct RenameFault {
    perform: bool,
    stat: Option<i32>,
}
impl FileSystem for RenameFault {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if to.parent().and_then(Path::file_name) != Some("done".as_ref()) {
            return Host.rename(from, to);
        }
        if self.perform {
            Host.rename(from, to)?;
        }
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
    fn stat_directory(&self, path: &Path) -> io::Result<()> {
        match self.stat {
            Some(errno) => Err(io::Error::from_raw_os_error(errno)),
            None => Host.stat_directory(path),
        }
    }
}
#[derive(Default)]
struct Seen(Mutex<Option<(Option<Reconcile>, Option<u32>)>>);
impl Observer for Seen {
    fn handed_over(&self, h: &HandOver) {
        *self.0.lock().unwrap() = Some((None, h.pending));
    }
    fn finished(&self, f: &Finished) {
        self.0.lock().unwrap().as_mut().unwrap().0 = f.reconcile;
    }
}
/// One texture input, its spool and sink under `scratch`.
fn one_input(scratch: &Scratch) -> phase1::PreparedRun {
    for name in ["spool", "sink"] {
        fs::create_dir(scratch.join(name)).unwrap();
    }
    let file = ngui::file(
        &[],
        &[ngui::texture(1, b"t", 4, (1, 1), vec![1, 2, 3, 255])],
        true,
    )
    .unwrap();
    fs::write(
        scratch.join("a"),
        unityrs_testkit::env::bundle(&[("CAB-a", file)]),
    )
    .unwrap();
    let mut spec = common::base();
    spec["threads"] = 1.into();
    spec["download_concurrency"] = 1.into();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["inputs"] = json!([{"id":"a","path":scratch.join("a")}]);
    spec["dependencies"] = json!([]);
    spec["outputs"] = json!({"textures":{"key":"{input}/{name}"}});
    phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
        .unwrap()
}
fn head(scratch: &Scratch) -> Option<Value> {
    let bytes = fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).ok()?;
    Some(serde_json::from_slice(&bytes).unwrap())
}
fn input_row(scratch: &Scratch) -> Value {
    let text = fs::read_to_string(scratch.join("sink/_unityrs/r1/1/inputs.0.ndjson")).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

#[test]
fn one_stat_reconciles_the_pending_commit() {
    // (rename landed, stat errno) -> the reconciliation and the input's row.
    let cases = [
        (true, None, Reconcile::Found(0), "complete", None),
        (
            false,
            None,
            Reconcile::Absent(0),
            "not_run",
            Some("stopped"),
        ),
        (
            true,
            Some(libc::EIO),
            Reconcile::Unknown(0, Some(libc::EIO)),
            "not_run",
            Some("stopped"),
        ),
        (
            false,
            Some(libc::EIO),
            Reconcile::Unknown(0, Some(libc::EIO)),
            "not_run",
            Some("stopped"),
        ),
        // The kernel's ENOMEM is the stat's answer, not our refusal.
        (
            true,
            Some(libc::ENOMEM),
            Reconcile::Unknown(0, Some(libc::ENOMEM)),
            "not_run",
            Some("stopped"),
        ),
    ];
    for (perform, stat, reconcile, status, reason) in cases {
        let scratch = Scratch::new("reconcile");
        let prepared = one_input(&scratch);
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let seen = Arc::new(Seen::default());
        let end = run::drive_with(
            prepared,
            0,
            &service.sender(),
            (&Space, Arc::new(publication_support::Portable)),
            &Options {
                observer: seen.clone(),
                spool: Some(Arc::new(RenameFault { perform, stat })),
                ..Options::default()
            },
        )
        .unwrap();
        let case = format!("perform {perform} stat {stat:?}");
        // The rename's own failure is the stop: host_io, 74.
        assert_eq!(
            (end.code, end.status, end.reason),
            (Code::Host, Some("incomplete"), Some("host_io")),
            "{case}"
        );
        assert_eq!(
            *seen.0.lock().unwrap(),
            Some((Some(reconcile), Some(0))),
            "{case}"
        );
        assert_eq!(scratch.join("spool/runs/r1/done/0").exists(), perform);
        let row = input_row(&scratch);
        assert_eq!(row["status"], status, "{case}");
        assert_eq!(row["reason"].as_str(), reason, "{case}");
        let head = head(&scratch).unwrap();
        assert_eq!(head["stop_reason"], "host_io");
        assert_eq!(head["phase"], "extract");
        assert_eq!(
            head["counts"]["inputs_complete"],
            u64::from(status == "complete")
        );
    }
}

const REASONS: [StopReason; 10] = [
    StopReason::Deadline,
    StopReason::Sigterm,
    StopReason::Sigint,
    StopReason::Sighup,
    StopReason::ParentDied,
    StopReason::StdinEof,
    StopReason::SpoolFull,
    StopReason::HostIo,
    StopReason::HostMemory,
    StopReason::Internal,
];
fn stop(reason: StopReason) -> Stop {
    Stop {
        detail: (reason == StopReason::SpoolFull).then_some(SpoolDetail::Reserve),
        ..Stop::external(reason)
    }
}
/// `78 > 69 > 70 > 74 > 7 > 6 > 5 > 4 > 3 > 0`, written out here rather
/// than read from the crate's table.
fn rank(code: Code) -> usize {
    [78, 69, 70, 74, 7, 6, 5, 4, 3, 0]
        .iter()
        .position(|&v| v == code.value())
        .unwrap()
}
fn finish_once(h: &HandOver, taken: Option<Stop>) -> Finished {
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 0,
        spool_peak: 0,
        vm_hwm: 0,
    };
    finish::run(
        h,
        taken,
        &mut finish::Finisher {
            account: &account,
            identity: &identity,
            budget: finish::budget::Budget::UNARMED,
            clock: &finish::budget::BootClock,
            backend: &mut finish::budget::NoJournal,
        },
    )
}

#[test]
fn a_terminal_code_yields_only_to_a_more_severe_stop() {
    let terminals = [
        (Code::BadSpec, "bad_spec"),
        (Code::CouldNotStart, "startup_failed"),
        (Code::Internal, "internal"),
        (Code::Host, "host_io"),
        (Code::Superseded, "superseded"),
    ];
    for (code, reason) in terminals {
        let h = HandOver::new(
            FinishCause::Terminal(Terminal { code, reason }),
            Phase::Start,
        );
        let alone = finish_once(&h, None).end;
        assert_eq!(
            (alone.code, alone.reason, alone.status),
            (code, Some(reason), None)
        );
        for r in REASONS {
            let taken = stop(r);
            let end = finish_once(&h, Some(taken)).end;
            let stop_wins = rank(r.code()) < rank(code);
            let want = if stop_wins { r.code() } else { code };
            assert_eq!(end.code, want, "{code:?} against {r:?}");
            assert_eq!(finish::reduce(code, Some(taken)), want);
            // The exit line names the reason that set the code.
            assert_eq!(
                end.reason,
                Some(if stop_wins { r.name() } else { reason }),
                "{code:?} against {r:?}"
            );
            assert_eq!(
                end.detail,
                (stop_wins && r == StopReason::SpoolFull).then_some("reserve")
            );
            assert_eq!(end.status, None);
        }
    }
    // 69 yields to none; 74 only to 70; 7 to the three host and internal
    // stops, never to a 6 or a 5.
    let yields = |code| {
        REASONS
            .iter()
            .filter(|r| rank(r.code()) < rank(code))
            .count()
    };
    assert_eq!(
        [Code::CouldNotStart, Code::Host, Code::Superseded].map(yields),
        [0, 1, 3]
    );
}

#[test]
fn a_stop_before_the_running_head_writes_no_head_and_exits_with_its_code() {
    for r in REASONS {
        let h = HandOver::new(FinishCause::Stop, Phase::Start);
        let end = finish_once(&h, Some(stop(r))).end;
        assert_eq!(
            (end.code, end.status, end.reason),
            (r.code(), None, Some(r.name()))
        );
    }
}

#[test]
fn a_finish_after_running_without_its_parts_leaves_running() {
    for r in REASONS {
        let mut h = HandOver::new(FinishCause::Stop, Phase::Start);
        h.running = true;
        let end = finish_once(&h, Some(stop(r))).end;
        let code = finish::reduce(Code::Internal, Some(stop(r)));
        let by_stop = code == r.code();
        assert_eq!(
            (end.code, end.status, end.reason),
            (
                code,
                Some("running"),
                Some(if by_stop { r.name() } else { "internal" })
            ),
            "{r:?}"
        );
        assert_eq!(
            end.detail,
            (by_stop && r == StopReason::SpoolFull).then_some("reserve")
        );
    }
    let mut h = HandOver::new(FinishCause::Normal, Phase::Publish);
    h.running = true;
    let end = finish_once(&h, None).end;
    assert_eq!(
        (end.code, end.status, end.reason),
        (Code::Internal, Some("running"), Some("internal"))
    );
}

/// The spool's filesystem, whose reads of committed records fail `EIO`
/// once the controller has handed over: only the finisher sees it.
struct LateReads(Arc<AtomicBool>);
impl FileSystem for LateReads {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn io::Read + Send>> {
        let done = path.components().any(|c| c.as_os_str() == "done");
        if done && self.0.load(Ordering::SeqCst) {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Host.open(path)
    }
}
/// Stops before phase 6's task, then, when `fault`, turns the record
/// reads' fault on (`armed`).
struct StopAtSelection {
    lifetime: Arc<Lifetime>,
    fault: bool,
    armed: Arc<AtomicBool>,
}
impl Observer for StopAtSelection {
    fn seam(&self, seam: Seam) {
        if seam == Seam::Selection {
            self.lifetime.request(Stop::external(StopReason::Sigterm));
        }
    }
    fn handed_over(&self, _: &HandOver) {
        self.armed.store(self.fault, Ordering::SeqCst);
    }
}

#[test]
fn a_stopped_finisher_that_cannot_read_its_records_leaves_running() {
    for fault in [false, true] {
        let scratch = Scratch::new("unread");
        let prepared = one_input(&scratch);
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let lifetime = Arc::new(Lifetime::default());
        let armed = Arc::new(AtomicBool::new(false));
        let end = run::drive_with(
            prepared,
            0,
            &service.sender(),
            (&Space, Arc::new(publication_support::Portable)),
            &Options {
                lifetime: lifetime.clone(),
                observer: Arc::new(StopAtSelection {
                    lifetime,
                    fault,
                    armed: armed.clone(),
                }),
                spool: Some(Arc::new(LateReads(armed))),
                ..Options::default()
            },
        )
        .unwrap();
        let head = head(&scratch).unwrap();
        if fault {
            // The read's 74 meets the stop's 6; `running` stays the head.
            assert_eq!(
                (end.code, end.status, end.reason),
                (Code::Host, Some("running"), Some("host_io"))
            );
            assert_eq!(head["status"], "running");
            assert!(!scratch.join("sink/_unityrs/r1/1/inputs.0.ndjson").exists());
        } else {
            assert_eq!(
                (end.code, end.status, end.reason),
                (Code::Stopped, Some("incomplete"), Some("sigterm"))
            );
            assert_eq!(head["status"], "incomplete");
            assert_eq!(head["stop_reason"], "sigterm");
            assert_eq!(head["phase"], "winners");
        }
    }
}

#[test]
fn a_stop_racing_startup_exits_with_its_own_code_whenever_no_head_was_written() {
    let run = |delay: Option<Duration>| {
        let scratch = Scratch::new("race");
        let prepared = one_input(&scratch);
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let lifetime = Arc::new(Lifetime::default());
        let sigterm = Stop::external(StopReason::Sigterm);
        let stopper = delay.map(|delay| {
            let lifetime = lifetime.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                lifetime.request(sigterm);
            })
        });
        if delay.is_some_and(|d| d.is_zero()) {
            // The first instant: the stop is live before the drive begins.
            lifetime.request(sigterm);
        }
        let started = Instant::now();
        let end = run::drive_with(
            prepared,
            0,
            &service.sender(),
            (&Space, Arc::new(publication_support::Portable)),
            &Options {
                lifetime,
                ..Options::default()
            },
        );
        let took = started.elapsed();
        if let Some(stopper) = stopper {
            stopper.join().unwrap();
        }
        (end, head(&scratch), took)
    };
    let (end, _, whole) = run(None);
    assert_eq!(end.unwrap().code, Code::Complete);
    let instants = 100_u32;
    let mut headless = 0;
    for i in 0..instants {
        let (end, head, _) = run(Some(whole * i / instants));
        let end = end.unwrap_or_else(|e| panic!("instant {i}: {e:?}"));
        match head {
            None => {
                headless += 1;
                assert_eq!(
                    (end.code, end.status, end.reason),
                    (Code::Stopped, None, Some("sigterm")),
                    "instant {i}"
                );
            }
            Some(head) => assert_eq!(head["status"], end.status.unwrap(), "instant {i}"),
        }
        if i == 0 {
            assert_eq!(headless, 1, "a stop before the drive writes no head");
        }
    }
    println!("RACE instants={instants} headless={headless} whole={whole:?}");
}
