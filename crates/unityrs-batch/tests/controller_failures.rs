mod artifact_support;
mod controller_support;
mod stage_support;
use controller_support::*;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use unityrs_batch::{
    attempt,
    controller::{Boundary, NoHooks, Point},
    spool::{Counter, FileSystem, Host, Writer},
};
use unityrs_testkit::guard;

#[test]
fn failed_attempt_candidates_are_removed_and_failure_figures_match_the_disk() {
    let () = guard::finishes("failed attempt cleanup and summary", || {
        let scene = unityrs_testkit::ngui::scenes()
            .unwrap()
            .into_iter()
            .find(|(name, _)| *name == "json_bytes_with_textures")
            .unwrap()
            .1;
        assert_eq!(scene.inputs.len(), 1);
        let mut solo = scene_harness(&scene, 1, 1);
        let mut controller = solo.controller(Arc::new(NoHooks));
        assert_eq!(
            controller.drive_local(solo.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        let cap = controller.state().inputs[0]
            .received
            .unwrap()
            .figures
            .export_work
            - 1;
        solo.execution.as_mut().unwrap().quiesce().unwrap();
        let mut h = scene_harness(&scene, 2, 2);
        h.configure(|s| s.max_export_work = cap);
        let copied = Arc::new(AtomicUsize::new(0));
        let seen = copied.clone();
        let hook = Arc::new(Tap {
            point: Box::new(move |p| {
                if matches!(p, Point::Attempt(_, attempt::Point::Copied(_))) {
                    seen.fetch_add(1, Ordering::AcqRel);
                }
            }),
            ..Tap::default()
        });
        let mut c = h.controller(hook);
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert!(
            copied.load(Ordering::Acquire) > 0,
            "failure follows real candidate writes"
        );
        assert!(!h.scratch.join("attempts/0").exists());
        assert_eq!(fs::read_dir(h.scratch.join("done/0")).unwrap().count(), 1);
        let rows = h.records(0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "failed");
        assert!(c.failure_writer_peak() > 0);
        assert!(c.failure_writer_peak() <= c.state().spec.max_decoded_bytes);
        let facts = c.state().inputs[0].received.unwrap();
        assert_eq!(facts.state, attempt::State::Failed);
        assert_eq!(rows[0]["export_work"], facts.figures.export_work);
        assert_eq!(rows[0]["spooled_part"], facts.spooled_part);
        assert_eq!(rows[0]["claimed_bytes"], facts.claimed_bytes);
        assert_eq!(rows[0]["ms"], facts.figures.ms);
        assert_eq!(rows[0]["skips"], 0);
        assert_eq!(rows[0]["unresolved_scripts"], 0);
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

struct NoSpace;
impl FileSystem for NoSpace {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(0)
    }
}
struct DeleteFault;
impl FileSystem for DeleteFault {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn remove_stoppable(
        &self,
        _: &std::fs::File,
        _: &Path,
        _: &dyn Fn() -> bool,
    ) -> Result<(), unityrs_batch::spool::Error> {
        Err(io::Error::from_raw_os_error(libc::EDQUOT).into())
    }
}

#[test]
fn spool_io_cap_and_reserve_refusals_keep_exact_details_and_commit_nothing() {
    let () = guard::finishes("spool refusal details", || {
        for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
            for operation in [1, 2, 3, 5] {
                let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
                let faults = artifact_support::Faults::default();
                faults.set(operation, errno);
                h.spool.fs = Arc::new(faults);
                let mut c = h.controller(Arc::new(NoHooks));
                assert_eq!(
                    c.drive_local(h.execution.as_mut().unwrap()),
                    Boundary::HandOver
                );
                let failure = c.failure().unwrap();
                assert_eq!(
                    failure.reason,
                    if errno == libc::EIO {
                        attempt::ReasonCode::HostIo
                    } else {
                        attempt::ReasonCode::SpoolFull
                    }
                );
                assert_eq!(
                    failure.detail.map(attempt::DetailCode::name),
                    match errno {
                        libc::ENOSPC => Some("ENOSPC"),
                        libc::EDQUOT => Some("EDQUOT"),
                        _ => None,
                    }
                );
                assert_eq!(fs::read_dir(h.scratch.join("done")).unwrap().count(), 0);
                h.execution.as_mut().unwrap().quiesce().unwrap();
            }
        }
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        h.spool.counter = Counter::new(1, 0, 4_096).unwrap();
        let mut c = h.controller(Arc::new(NoHooks));
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::HandOver
        );
        assert_eq!(
            c.failure().unwrap().detail,
            Some(attempt::DetailCode::SizeBytes)
        );
        h.execution.as_mut().unwrap().quiesce().unwrap();
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        h.spool.counter = Counter::new(1 << 30, 1, 4_096).unwrap();
        h.spool.fs = Arc::new(NoSpace);
        let mut c = h.controller(Arc::new(NoHooks));
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::HandOver
        );
        assert_eq!(
            c.failure().unwrap().detail,
            Some(attempt::DetailCode::Reserve)
        );
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

struct HoldWrite {
    reached: mpsc::Sender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
struct HeldWriter {
    file: Box<dyn Writer>,
    reached: mpsc::Sender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
impl FileSystem for HoldWrite {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        let file = Host.create(path)?;
        if path.components().any(|p| p.as_os_str() == "fail") {
            Ok(Box::new(HeldWriter {
                file,
                reached: self.reached.clone(),
                release: self.release.clone(),
            }))
        } else {
            Ok(file)
        }
    }
}
impl Write for HeldWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.reached.send(()).unwrap();
        guard::hold(&self.release.lock().unwrap(), "held file write");
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Writer for HeldWriter {
    fn seal(&self) -> io::Result<()> {
        self.file.seal()
    }
}
#[test]
fn held_failure_file_completion_after_handover_creates_no_done_artifact() {
    let () = guard::finishes("late failure file completion", || {
        let mut h = Harness::new(&[("a", b"bad".to_vec(), vec![])], &[], 1, 1);
        let stop = h.control.clone();
        let (reached, receive) = mpsc::channel();
        let (release, held) = mpsc::channel();
        h.spool.fs = Arc::new(HoldWrite {
            reached,
            release: Arc::new(Mutex::new(held)),
        });
        let mut c = h.controller(Arc::new(NoHooks));
        let wake = h.execution.as_ref().unwrap().wake();
        let (done, finished) = mpsc::channel();
        let driver = std::thread::spawn(move || {
            let boundary = c.drive_local(h.execution.as_mut().unwrap());
            done.send((h, c, boundary)).unwrap();
        });
        guard::wait(&receive, "failure file write entered");
        stop.store(true, Ordering::Release);
        wake.notify().unwrap();
        let (mut h, c, boundary) = guard::recv(&finished, "handover before write returns");
        assert_eq!(boundary, Boundary::HandOver);
        assert_eq!(c.state().uncommitted.get(0), Some(true));
        release.send(()).unwrap();
        h.execution.as_mut().unwrap().quiesce().unwrap();
        assert!(
            fs::metadata(h.scratch.join("fail/0/records.ndjson"))
                .unwrap()
                .len()
                > 0
        );
        assert!(!h.scratch.join("done/0").exists());
        driver.join().unwrap();
    });
}

#[test]
fn cooperative_delete_error_preserves_partial_attempt_and_never_writes_failure_record() {
    let () = guard::finishes("delete failure", || {
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        h.configure(|s| s.max_export_work = 0);
        h.spool.fs = Arc::new(DeleteFault);
        let mut c = h.controller(Arc::new(NoHooks));
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::HandOver
        );
        assert_eq!(
            c.failure().unwrap().detail,
            Some(attempt::DetailCode::Edquot)
        );
        assert!(h.scratch.join("attempts/0").is_dir());
        assert!(!h.scratch.join("fail/0").exists());
        assert!(!h.scratch.join("done/0").exists());
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

#[test]
fn a_failure_writer_fits_its_vacancy_beside_another_reserved_job() {
    let () = guard::finishes("failure writer beside reserved job", || {
        let mut scene = unityrs_testkit::ngui::scenes()
            .unwrap()
            .into_iter()
            .find(|(name, _)| *name == "json_bytes_with_textures")
            .unwrap()
            .1;
        let mut solo = scene_harness(&scene, 1, 1);
        let mut baseline = solo.controller(Arc::new(NoHooks));
        assert_eq!(
            baseline.drive_local(solo.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        let cap = baseline.state().inputs[0]
            .received
            .unwrap()
            .figures
            .export_work
            - 1;
        solo.execution.as_mut().unwrap().quiesce().unwrap();
        scene.bundles.push(unityrs_testkit::ngui::SceneBundle {
            id: "small".to_owned(),
            data: clear(1),
            deps: vec![],
        });
        scene.inputs.push("small".to_owned());
        let mut h = scene_harness(&scene, 2, 2);
        h.configure(|s| s.max_export_work = cap);
        let (reached, receive) = mpsc::channel();
        let receive = Mutex::new(receive);
        let (release, held) = mpsc::channel();
        let held = Mutex::new(held);
        let dispatches = Arc::new(AtomicUsize::new(0));
        let running = dispatches.clone();
        let hooks = Arc::new(Tap {
            event: Box::new(move |event| {
                if matches!(
                    event,
                    unityrs_batch::controller::Event::AttemptDispatched(_)
                ) {
                    running.fetch_add(1, Ordering::AcqRel);
                }
                if event == unityrs_batch::controller::Event::Renamed(0) {
                    guard::wait(&receive.lock().unwrap(), "other reservation held");
                    release.send(()).unwrap();
                }
            }),
            point: Box::new(move |point| {
                if point == Point::BeforeSend(unityrs_batch::phase1::JobKind::Attempt, 1) {
                    reached.send(()).unwrap();
                    guard::hold(&held.lock().unwrap(), "failure commit releases peer");
                }
                if point == Point::FailureCreated(0) {
                    assert_eq!(dispatches.load(Ordering::Acquire), 2);
                }
            }),
        });
        let mut c = h.controller(hooks);
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert!(
            c.failure_writer_peak() > 0
                && c.failure_writer_peak() <= c.state().spec.max_decoded_bytes
        );
        assert_eq!(h.records(0)[0]["status"], "failed");
        assert_eq!(h.records(1)[0]["status"], "complete");
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
