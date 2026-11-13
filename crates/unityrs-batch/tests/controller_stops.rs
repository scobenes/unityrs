mod controller_support;
mod stage_support;
use controller_support::*;
use std::{
    fs, io,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use unityrs_batch::{
    controller::{Boundary, Point},
    peek::{self, word::Outcome},
    phase1::JobKind,
    spool::{FileSystem, Host},
};
use unityrs_testkit::guard;

#[test]
fn cooperative_stop_at_each_local_seam_freezes_received_facts_and_bits() {
    let () = guard::finishes("all cooperative seams", || {
        let points = [
            Point::Stage(0, unityrs_batch::stage::local::Point::Buffer(0)),
            Point::Peek(0, peek::Point::Open),
            Point::Peek(0, peek::Point::AfterWord),
            Point::BeforeSend(JobKind::Attempt, 0),
            Point::BeforeCommitLoad(0),
            Point::AfterRenameTrace(0),
        ];
        for point in points {
            let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
            let stop = h.control.clone();
            let hook = Arc::new(Tap {
                point: Box::new(move |p| {
                    if p == point {
                        stop.store(true, Ordering::Release);
                    }
                }),
                ..Tap::default()
            });
            let mut c = h.controller(hook);
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::HandOver,
                "{point:?}"
            );
            match point {
                Point::Peek(_, peek::Point::AfterWord) => {
                    assert!(matches!(
                        c.state().peek_words[0].load(),
                        Ok(Outcome::Clear(_))
                    ));
                    assert!(c.state().bundles[0].peek.is_none());
                    assert_eq!(c.state().uncommitted.get(0), Some(false));
                }
                Point::BeforeCommitLoad(_) => {
                    assert!(c.state().inputs[0].received.is_some());
                    assert_eq!(c.state().uncommitted.get(0), Some(true));
                    assert_eq!(c.pending_commit(), None);
                }
                Point::AfterRenameTrace(_) => {
                    assert_eq!(c.state().uncommitted.get(0), Some(false));
                    assert_eq!(h.records(0)[0]["status"], "complete");
                }
                Point::BeforeSend(JobKind::Attempt, _) => {
                    assert!(c.state().inputs[0].received.is_none());
                    assert_eq!(c.state().uncommitted.get(0), Some(true));
                }
                _ => assert!(c.state().inputs[0].received.is_none()),
            }
            if point != Point::AfterRenameTrace(0) {
                assert!(!h.scratch.join("done/0").exists());
            }
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

#[test]
fn stop_during_either_failure_barrier_leaves_every_uncommitted_holder_marked() {
    let () = guard::finishes("failure barrier stop", || {
        for stage in [false, true] {
            let mut h = Harness::new(
                &[
                    ("a", b"bad".to_vec(), vec![]),
                    ("b", b"bad".to_vec(), vec![]),
                ],
                &[],
                2,
                2,
            );
            if stage {
                for i in 0..2 {
                    fs::remove_file(h.source(i)).unwrap();
                }
            }
            let stop = h.control.clone();
            let hook = Arc::new(Tap {
                point: Box::new(move |p| {
                    if p == Point::BeforeCommitLoad(0) {
                        stop.store(true, Ordering::Release);
                    }
                }),
                ..Tap::default()
            });
            let mut c = h.controller(hook);
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::HandOver
            );
            for i in 0..2 {
                assert_eq!(c.state().uncommitted.get(i), Some(true));
                assert!(c.state().inputs[i].received.is_none());
                assert!(!h.scratch.join(format!("done/{i}")).exists());
            }
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

#[test]
fn panic_at_both_commit_points_preserves_pending_owner_bits_and_received_facts() {
    let () = guard::finishes("commit panic", || {
        for after in [false, true] {
            let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
            let hook = Arc::new(Tap {
                point: Box::new(move |p| {
                    assert_ne!(
                        p,
                        if after {
                            Point::AfterRenameTrace(0)
                        } else {
                            Point::BeforeCommitLoad(0)
                        },
                        "commit fault"
                    );
                }),
                ..Tap::default()
            });
            let mut c = h.controller(hook);
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::HandOver
            );
            assert_eq!(
                c.failure().unwrap().reason,
                unityrs_batch::attempt::ReasonCode::Internal
            );
            assert_eq!(c.pending_commit(), Some(0));
            assert_eq!(c.state().uncommitted.get(0), Some(true));
            assert!(c.state().inputs[0].received.is_some());
            assert_eq!(h.scratch.join("done/0").exists(), after);
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

struct RenameFault {
    perform: bool,
    errno: i32,
}
impl FileSystem for RenameFault {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.perform {
            Host.rename(from, to)?;
        }
        Err(io::Error::from_raw_os_error(self.errno))
    }
}
#[test]
fn failed_and_perform_then_fail_renames_retain_the_pending_commit_for_reconciliation() {
    let () = guard::finishes("rename reconciliation", || {
        for perform in [false, true] {
            for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT] {
                let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
                h.spool.fs = Arc::new(RenameFault { perform, errno });
                let mut c = h.controller(Arc::new(unityrs_batch::controller::NoHooks));
                assert_eq!(
                    c.drive_local(h.execution.as_mut().unwrap()),
                    Boundary::HandOver
                );
                assert_eq!(c.pending_commit(), Some(0));
                assert_eq!(c.rename_failure(), Some((0, Some(errno))));
                assert_eq!(c.state().uncommitted.get(0), Some(true));
                assert_eq!(h.scratch.join("done/0").exists(), perform);
                assert_eq!(
                    c.failure()
                        .unwrap()
                        .detail
                        .map(unityrs_batch::attempt::DetailCode::name),
                    match errno {
                        libc::ENOSPC => Some("ENOSPC"),
                        libc::EDQUOT => Some("EDQUOT"),
                        _ => None,
                    }
                );
                h.execution.as_mut().unwrap().quiesce().unwrap();
            }
        }
    });
}

struct NotifyDrop(mpsc::Sender<()>);
impl Drop for NotifyDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[test]
fn a_held_failure_task_cannot_commit_after_handover_and_drop_does_not_join_it() {
    let () = guard::finishes("held failure task handover", || {
        let mut h = Harness::new(&[("a", b"bad".to_vec(), vec![])], &[], 1, 1);
        let stop = h.control.clone();
        let (reached, receive) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let held = Mutex::new(held);
        let (exited, worker_exit) = mpsc::channel();
        let lifetime = NotifyDrop(exited);
        let hook = Arc::new(Tap {
            point: Box::new(move |p| {
                let _lifetime = &lifetime;
                if p == Point::FailureCreated(0) {
                    reached.send(()).unwrap();
                    guard::hold(&held.lock().unwrap(), "failure write release");
                }
            }),
            ..Tap::default()
        });
        let mut c = h.controller(hook);
        let wake = h.execution.as_ref().unwrap().wake();
        let (done, finished) = mpsc::channel();
        let driver = std::thread::spawn(move || {
            let boundary = c.drive_local(h.execution.as_mut().unwrap());
            done.send((h, c, boundary)).unwrap();
        });
        guard::wait(&receive, "failure task entered");
        stop.store(true, Ordering::Release);
        wake.notify().unwrap();
        let (mut h, c, boundary) = guard::recv(&finished, "handover while pool held");
        assert_eq!(boundary, Boundary::HandOver);
        assert_eq!(c.state().uncommitted.get(0), Some(true));
        assert!(!h.scratch.join("done/0").exists());
        let execution = h.execution.take().unwrap();
        let () = guard::finishes("execution drop while failure task held", move || {
            drop(execution);
        });
        drop(c);
        release.send(()).unwrap();
        // All worker/task references to the hooks must be gone before scratch
        // cleanup. This also observes the held task completing after drop.
        guard::wait(&worker_exit, "detached execution finished");
        driver.join().unwrap();
        assert!(!h.scratch.join("done/0").exists());
    });
}

#[test]
fn a_wake_only_poll_still_drains_an_already_queued_end() {
    let () = guard::finishes("wake-only drain", || {
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        let seen = Arc::new(AtomicBool::new(false));
        let flag = seen.clone();
        let hook = Arc::new(Tap {
            point: Box::new(move |p| {
                if p == Point::PollReturned {
                    flag.store(true, Ordering::Release);
                }
            }),
            ..Tap::default()
        });
        let mut c = h.controller(hook);
        h.execution.as_ref().unwrap().wake().notify().unwrap();
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert!(seen.load(Ordering::Acquire));
        assert_eq!(h.records(0)[0]["status"], "complete");
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

#[test]
fn queued_ends_are_drained_on_a_wake_only_poll_and_omitted_at_handover() {
    let () = guard::finishes("send before end wake", || {
        for stopped in [false, true] {
            let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
            let stop = h.control.clone();
            let (queued, queued_rx) = mpsc::channel();
            let (release_worker, worker_rx) = mpsc::channel();
            let release_worker_root = release_worker.clone();
            let worker_rx = Mutex::new(worker_rx);
            let (polling, polling_rx) = mpsc::channel();
            let (release_poll, poll_rx) = mpsc::channel();
            let poll_rx = Mutex::new(poll_rx);
            let armed = Arc::new(AtomicBool::new(false));
            let flag = armed.clone();
            let hooks = Arc::new(Tap {
                event: Box::new(move |e| {
                    if e == unityrs_batch::controller::Event::PeekDispatched(0) {
                        flag.store(true, Ordering::Release);
                    }
                    if e == unityrs_batch::controller::Event::PeekReceived(0) {
                        release_worker.send(()).unwrap();
                    }
                }),
                point: Box::new(move |p| {
                    if p == Point::EndQueued(JobKind::Peek, 0) {
                        queued.send(()).unwrap();
                        guard::hold(&worker_rx.lock().unwrap(), "end wake release");
                    }
                    if p == Point::BeforePoll && armed.swap(false, Ordering::AcqRel) {
                        polling.send(()).unwrap();
                        guard::hold(&poll_rx.lock().unwrap(), "poll release");
                    }
                }),
            });
            let mut c = h.controller(hooks);
            let wake = h.execution.as_ref().unwrap().wake();
            let end_wake = h.execution.as_ref().unwrap().end_wake();
            let (done, done_rx) = mpsc::channel();
            let driver = std::thread::spawn(move || {
                let boundary = c.drive_local(h.execution.as_mut().unwrap());
                done.send((h, c, boundary)).unwrap();
            });
            guard::wait(&queued_rx, "end queued without its eventfd write");
            guard::wait(&polling_rx, "controller before poll");
            end_wake.drain().unwrap(); // remove staging's earlier wake
            stop.store(stopped, Ordering::Release);
            wake.notify().unwrap();
            release_poll.send(()).unwrap();
            let (mut h, c, boundary) = guard::recv(&done_rx, "wake-only result");
            assert_eq!(
                boundary,
                if stopped {
                    Boundary::HandOver
                } else {
                    Boundary::ReadyForWinners
                }
            );
            assert_eq!(c.state().bundles[0].peek.is_none(), stopped);
            if stopped {
                assert!(c.state().inputs[0].received.is_none());
                release_worker_root.send(()).unwrap();
            }
            h.execution.as_mut().unwrap().quiesce().unwrap();
            driver.join().unwrap();
        }
    });
}
