//! B8b's archive fault matrix through the directory adapter and
//! outcome faults: temp write, writeback and rename failures, interrupted and
//! short writes, a rename-time kind conflict, an outcome failure after the
//! rename, the rename-to-outcome window, and the pool's winner-to-archive
//! boundary.
mod common;
mod controller_support;
#[allow(dead_code)]
mod directory_support;
mod publication_support;
mod stage_support;
use directory_support::Faults;
use publication_support::Scene;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use unityrs_batch::{
    attempt::{DetailCode, ReasonCode},
    digest::Hashes,
    outcomes::{Code, Outcomes},
    phase::{Account, Error},
    publish::{self, Admission, Context, Gate, Hooks, Point, payload::Committed},
    sink::directory::Directory,
    spool::Host,
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

const KEYS: [&str; 2] = ["z.zip", "a.zip"];
const WINNERS: [&str; 3] = ["z", "a", "m"];

fn adapter(fail: Option<(&'static str, i32)>, interrupt: bool, short_write: bool) -> Arc<Faults> {
    Arc::new(Faults {
        fallback: Some(libc::ENOSYS),
        fail,
        disarmed: AtomicBool::new(true),
        interrupt,
        short_write,
        crossing: AtomicUsize::new(usize::MAX),
        ..Faults::default()
    })
}
fn directory(scene: &Scene, name: &str, fs: Arc<Faults>) -> Directory {
    let root = scene.scratch.join(name);
    std::fs::create_dir(&root).unwrap();
    directory_support::preflight(&root, 1, fs).unwrap()
}
fn drive(
    scene: &Scene,
    directory: &Directory,
    outcomes: &Outcomes,
    gate: &Gate,
    hooks: &dyn Hooks,
    stop: &AtomicBool,
) -> Result<Admission, Error> {
    let account = Account::new(64 << 20);
    let result = publish::drive(&Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        directory,
        outcomes,
        gate,
        account: &account,
        run_id: "r",
        resume: false,
        stopped: &|| stop.load(Ordering::SeqCst),
        hooks,
    });
    assert_eq!(account.snapshot().input.used, 0);
    result
}
/// Archive temps are `.unityrs-tmp-r-<winners + spec index>`.
fn archive_temps(scene: &Scene, root: &str) -> Vec<String> {
    std::fs::read_dir(scene.scratch.join(root))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.strip_prefix(".unityrs-tmp-r-")
                .is_some_and(|n| n.parse::<u64>().unwrap() >= scene.handoff.counts.winners)
        })
        .collect()
}
/// Clean archive bytes and the archive phase's write-call count.
fn clean() -> (Vec<Vec<u8>>, usize) {
    let scene = Scene::new(&WINNERS, true, 4096);
    let faults = adapter(None, false, false);
    let dir = directory(&scene, "sink", faults.clone());
    let out = scene.outcomes("out", &Host);
    let stop = AtomicBool::new(false);
    drive(&scene, &dir, &out, &Gate::default(), &arm(&faults), &stop).unwrap();
    let writes = faults
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| **c == "write")
        .count();
    let bytes = KEYS
        .iter()
        .map(|key| std::fs::read(scene.scratch.join(format!("sink/{key}"))).unwrap())
        .collect();
    (bytes, writes)
}

/// Arms the adapter (and optionally outcome faults) when the first archive
/// reaches its transfer point, after every loose winner has published, and
/// optionally runs `at` once at a later archive point.
struct Arm<'a> {
    faults: &'a Faults,
    outcomes: Option<&'a AtomicBool>,
    transfers: AtomicUsize,
    at: Option<(Point, &'a (dyn Fn() + Sync))>,
}
impl Hooks for Arm<'_> {
    fn point(&self, point: Point) {
        if point == Point::Transfer
            && self.transfers.fetch_add(1, Ordering::SeqCst) == WINNERS.len()
        {
            self.faults.calls.lock().unwrap().clear();
            self.faults.disarmed.store(false, Ordering::SeqCst);
            if let Some(armed) = self.outcomes {
                armed.store(true, Ordering::SeqCst);
            }
        }
        if let Some((at, run)) = self.at
            && at == point
            && self.transfers.load(Ordering::SeqCst) > WINNERS.len()
        {
            run();
        }
    }
}
fn arm(faults: &Faults) -> Arm<'_> {
    Arm {
        faults,
        outcomes: None,
        transfers: AtomicUsize::new(0),
        at: None,
    }
}

#[test]
fn temp_write_writeback_and_rename_failures_stop_the_run_and_remove_the_temp() {
    for operation in ["write", "writeback", "rename"] {
        for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
            let scene = Scene::new(&WINNERS, true, 4096);
            let faults = adapter(Some((operation, errno)), false, false);
            let dir = directory(&scene, "sink", faults.clone());
            let out = scene.outcomes("out", &Host);
            let stop = AtomicBool::new(false);
            let result = drive(&scene, &dir, &out, &Gate::default(), &arm(&faults), &stop);
            assert!(
                matches!(result, Err(Error::Run(f)) if f.reason == ReasonCode::HostIo),
                "{operation} {errno}: {result:?}"
            );
            for index in 0..3 {
                assert_eq!(out.loose(index).unwrap(), Code::Written);
            }
            for index in 0..2 {
                assert_eq!(out.archive(index).unwrap().code, Code::NotWritten);
            }
            for key in KEYS {
                assert!(!scene.scratch.join(format!("sink/{key}")).exists());
            }
            assert_eq!(archive_temps(&scene, "sink"), Vec::<String>::new());
            let calls = faults.calls.lock().unwrap().clone();
            assert_eq!(calls.last(), Some(&"unlink"), "{operation}: {calls:?}");
        }
    }
}

#[test]
fn interrupted_and_short_temp_writes_publish_identical_archives() {
    let (expected, baseline) = clean();
    for (interrupt, short_write) in [(true, false), (false, true)] {
        let scene = Scene::new(&WINNERS, true, 4096);
        let faults = adapter(None, interrupt, short_write);
        let dir = directory(&scene, "sink", faults.clone());
        let out = scene.outcomes("out", &Host);
        let stop = AtomicBool::new(false);
        drive(&scene, &dir, &out, &Gate::default(), &arm(&faults), &stop).unwrap();
        for (index, key) in KEYS.iter().enumerate() {
            let bytes = std::fs::read(scene.scratch.join(format!("sink/{key}"))).unwrap();
            assert_eq!(bytes, expected[index], "{key} {interrupt} {short_write}");
            let record = out.archive(index as u64).unwrap();
            assert_eq!(record.code, Code::Written);
            assert_eq!(record.md5, Hashes::of(&bytes).md5);
        }
        let calls = faults.calls.lock().unwrap().clone();
        let writes = calls.iter().filter(|c| **c == "write").count();
        if short_write {
            // One byte per write call.
            let total: usize = expected.iter().map(Vec::len).sum();
            assert_eq!(writes, total);
        } else {
            // One EINTR on the first archive write, then one retry.
            assert_eq!(writes, baseline + 1);
        }
        assert_eq!(archive_temps(&scene, "sink"), Vec::<String>::new());
    }
}

#[test]
fn a_directory_appearing_before_the_rename_fails_only_that_archive_without_a_temp() {
    let scene = Scene::new(&WINNERS, true, 4096);
    let faults = adapter(None, false, false);
    let dir = directory(&scene, "sink", faults.clone());
    let out = scene.outcomes("out", &Host);
    let stop = AtomicBool::new(false);
    // The pre-check passed; the kind conflict appears after generation.
    let target = scene.scratch.join("sink/a.zip");
    let once = AtomicBool::new(false);
    let conflict = || {
        if !once.swap(true, Ordering::SeqCst) {
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("child"), b"x").unwrap();
        }
    };
    let hooks = Arm {
        at: Some((Point::BeforeCommit, &conflict)),
        ..arm(&faults)
    };
    drive(&scene, &dir, &out, &Gate::default(), &hooks, &stop).unwrap();
    let failed = out.archive(1).unwrap();
    assert_eq!(failed.code, Code::PublishFailed);
    assert_eq!((failed.size, failed.md5), (0, [0; 16]));
    assert_eq!(out.archive(0).unwrap().code, Code::Written);
    assert!(target.join("child").is_file());
    assert_eq!(archive_temps(&scene, "sink"), Vec::<String>::new());
    let calls = faults.calls.lock().unwrap().clone();
    let rename = calls.iter().position(|c| *c == "rename").unwrap();
    assert_eq!(calls[rename + 1], "unlink", "{calls:?}");
}

#[test]
fn an_outcome_failure_after_the_rename_is_spool_full_and_resume_finds_it_present() {
    let scene = Scene::new(&WINNERS, true, 4096);
    let faults = adapter(None, false, false);
    let dir = directory(&scene, "sink", faults.clone());
    let armed = Arc::new(AtomicBool::new(false));
    let out = scene.outcomes("out", &publication_support::OutcomeFaults(armed.clone()));
    let stop = AtomicBool::new(false);
    let hooks = Arm {
        outcomes: Some(&armed),
        ..arm(&faults)
    };
    let result = drive(&scene, &dir, &out, &Gate::default(), &hooks, &stop);
    assert!(
        matches!(result, Err(Error::Run(f))
            if f.reason == ReasonCode::SpoolFull && f.detail == Some(DetailCode::Enospc)),
        "{result:?}"
    );
    // Renamed but unacknowledged: the outcome stays not_written.
    let renamed = std::fs::read(scene.scratch.join("sink/a.zip")).unwrap();
    armed.store(false, Ordering::SeqCst);
    assert_eq!(out.archive(1).unwrap().code, Code::NotWritten);
    assert_eq!(archive_temps(&scene, "sink"), Vec::<String>::new());
    let out = scene.outcomes("out2", &Host);
    let account = Account::new(64 << 20);
    publish::drive(&Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        directory: &dir,
        outcomes: &out,
        gate: &Gate::default(),
        account: &account,
        run_id: "r",
        resume: true,
        stopped: &|| false,
        hooks: &publish::NoHooks,
    })
    .unwrap();
    let record = out.archive(1).unwrap();
    assert_eq!(record.code, Code::Present);
    assert_eq!(record.md5, Hashes::of(&renamed).md5);
    assert_eq!(out.archive(0).unwrap().code, Code::Written);
}

/// Holds the first archive between its authorized rename and its outcome.
struct Window<'a> {
    transfers: AtomicUsize,
    held: &'a AtomicBool,
    release: &'a AtomicBool,
}
impl Hooks for Window<'_> {
    fn point(&self, point: Point) {
        if point == Point::Transfer {
            self.transfers.fetch_add(1, Ordering::SeqCst);
        }
        if point == Point::Outcome
            && self.transfers.load(Ordering::SeqCst) == WINNERS.len() + 1
            && !self.held.swap(true, Ordering::SeqCst)
        {
            let start = std::time::Instant::now();
            while !self.release.load(Ordering::SeqCst) {
                assert!(start.elapsed() < DEADLOCK_GUARD, "window never released");
                std::thread::yield_now();
            }
        }
    }
}
#[test]
fn a_stop_and_a_freeze_inside_the_rename_to_outcome_window_still_see_written() {
    let () = unityrs_testkit::guard::finishes("archive rename window", || {
        let scene = Scene::new(&WINNERS, true, 4096);
        let dir = directory(&scene, "sink", adapter(None, false, false));
        let out = scene.outcomes("out", &Host);
        let gate = Gate::default();
        let stop = AtomicBool::new(false);
        let (held, release) = (AtomicBool::new(false), AtomicBool::new(false));
        let hooks = Window {
            transfers: AtomicUsize::new(0),
            held: &held,
            release: &release,
        };
        let account = Account::new(64 << 20);
        let payloads = Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        };
        let stopped = || stop.load(Ordering::SeqCst);
        let context = Context {
            handoff: &scene.handoff,
            workspace: &scene.workspace,
            payloads: &payloads,
            directory: &dir,
            outcomes: &out,
            gate: &gate,
            account: &account,
            run_id: "r",
            resume: false,
            stopped: &stopped,
            hooks: &hooks,
        };
        std::thread::scope(|scope| {
            let thread = scope.spawn(move || publish::drive(&context));
            while !held.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            // Renamed under the gate, outcome not yet written.
            assert!(scene.scratch.join("sink/a.zip").is_file());
            assert_eq!(out.archive(1).unwrap().code, Code::NotWritten);
            stop.store(true, Ordering::SeqCst);
            release.store(true, Ordering::SeqCst);
            // The freeze waits for the gate: rename and outcome are one step.
            let frozen = gate.freeze(&out).unwrap();
            let bytes = std::fs::read(scene.scratch.join("sink/a.zip")).unwrap();
            let record = frozen.outcomes.archive(1).unwrap();
            assert_eq!(record.code, Code::Written);
            assert_eq!(record.md5, Hashes::of(&bytes).md5);
            drop(frozen);
            assert_eq!(thread.join().unwrap(), Err(Error::Stopped));
        });
        assert_eq!(out.archive(0).unwrap().code, Code::NotWritten);
        assert!(!scene.scratch.join("sink/z.zip").exists());
    });
}

/// Holds the last winner's transfer until the other winners' outcomes are
/// written, then lets an idle drive show whether it starts an archive.
#[derive(Default)]
struct Final {
    transfers: AtomicUsize,
    outcomes: AtomicUsize,
    held: AtomicBool,
    early: AtomicBool,
    lock: Mutex<()>,
    changed: Condvar,
}
impl Hooks for Final {
    fn point(&self, point: Point) {
        let guard = self.lock.lock().unwrap();
        match point {
            Point::Outcome => {
                self.outcomes.fetch_add(1, Ordering::SeqCst);
            }
            Point::Transfer => {
                let n = self.transfers.fetch_add(1, Ordering::SeqCst) + 1;
                if n == WINNERS.len() {
                    self.held.store(true, Ordering::SeqCst);
                    let (guard, waited) = self
                        .changed
                        .wait_timeout_while(guard, DEADLOCK_GUARD, |()| {
                            self.outcomes.load(Ordering::SeqCst) < WINNERS.len() - 1
                        })
                        .unwrap();
                    assert!(!waited.timed_out(), "other winners never finished");
                    // An idle drive gets a grace period to (wrongly) start an
                    // archive while this winner is still live.
                    let (_guard, _) = self
                        .changed
                        .wait_timeout_while(guard, std::time::Duration::from_millis(300), |()| {
                            !self.early.load(Ordering::SeqCst)
                        })
                        .unwrap();
                    self.held.store(false, Ordering::SeqCst);
                    return;
                }
                if n > WINNERS.len() && self.held.load(Ordering::SeqCst) {
                    self.early.store(true, Ordering::SeqCst);
                }
            }
            _ => {}
        }
        drop(guard);
        self.changed.notify_all();
    }
}
#[test]
fn pool_archives_wait_for_the_final_winner_transfer_not_the_cursor() {
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 2, 2);
    let controller = harness.controller(Arc::new(controller_support::Observe::default()));
    let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
    let scene = Scene::new(&WINNERS, true, 4096);
    let sink = scene.scratch.join("sink");
    std::fs::create_dir(&sink).unwrap();
    let directory = directory_support::preflight(&sink, 1, directory_support::fs(true)).unwrap();
    let outcomes = scene.outcomes("out", &Host);
    let hooks = Arc::new(Final::default());
    let owned = Arc::new(publish::Owned {
        prepared: unityrs_batch::sys::Lock::new(scene.prepared),
        done: scene.done,
        workspace: scene.workspace,
        handoff: scene.handoff,
        directory: Arc::new(directory),
        outcomes,
        account: Arc::new(Account::new(64 << 20)),
        gate: Gate::default(),
        fs: Arc::new(Host),
        control: Arc::new(AtomicBool::new(false)),
        hooks: hooks.clone(),
        progress: None,
        spool_lease: None,
    });
    let report = publish::parallel(&owned, &execution, 2).unwrap();
    assert_eq!(report.written, 3);
    assert!(
        !hooks.early.load(Ordering::SeqCst),
        "an archive started while the final winner was still transferring"
    );
    assert_eq!(hooks.transfers.load(Ordering::SeqCst), 5);
    for index in 0..2 {
        assert_eq!(owned.outcomes.archive(index).unwrap().code, Code::Written);
    }
    execution.quiesce().unwrap();
}
