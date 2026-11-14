mod common;
mod controller_support;
#[allow(dead_code)]
mod directory_support;
mod stage_support;
use std::{
    fs,
    sync::{Arc, Condvar, Mutex, atomic::Ordering, mpsc},
};
use unityrs_batch::{
    controller::{Boundary, Hooks, Point},
    start,
};
use unityrs_testkit::{
    guard::{self, DEADLOCK_GUARD},
    scratch::Scratch,
};
struct Space;
impl start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
struct Hold {
    reached: mpsc::SyncSender<()>,
    released: Mutex<bool>,
    changed: Condvar,
}
impl unityrs_export::ngui::Hooks for Hold {}
impl Hooks for Hold {
    fn point(&self, point: Point) {
        if !matches!(
            point,
            Point::Stage(_, unityrs_batch::stage::local::Point::Buffer(0))
        ) {
            return;
        }
        let _ = self.reached.try_send(());
        let (_guard, waited) = self
            .changed
            .wait_timeout_while(self.released.lock().unwrap(), DEADLOCK_GUARD, |released| {
                !*released
            })
            .unwrap();
        assert!(!waited.timed_out());
    }
}
struct Release(Arc<Hold>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.changed.notify_all();
    }
}
#[test]
fn a_detached_stage_retains_the_spool_lock_after_the_host_returns() {
    let lease_scratch = Scratch::new("stage-lease");
    fs::create_dir(lease_scratch.join("spool")).unwrap();
    fs::create_dir(lease_scratch.join("sink")).unwrap();
    let mut spec = common::base();
    spec["run_id"] = "r".into();
    spec["inputs"] = serde_json::json!([{"id":"a","path":lease_scratch.join("source")}]);
    spec["dependencies"] = serde_json::json!([]);
    spec["spool"]["dir"] = serde_json::json!(lease_scratch.join("spool"));
    spec["sink"]["root"] = serde_json::json!(lease_scratch.join("sink"));
    let spec = unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap())
        .unwrap()
        .spec;
    let local = start::supported(&spec).unwrap();
    let started = Arc::new(start::spool::start(&local, &Space, &|| false).unwrap());
    let weak_started = Arc::downgrade(&started);
    let directory = Arc::new(
        directory_support::preflight(&lease_scratch.join("sink"), 1, directory_support::fs(true))
            .unwrap(),
    );
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 1, 1);
    harness.spool = harness.spool.clone().retained(started, directory);
    let (send, receive) = mpsc::sync_channel(1);
    let hooks = Arc::new(Hold {
        reached: send,
        released: Mutex::new(false),
        changed: Condvar::new(),
    });
    let weak_hooks = Arc::downgrade(&hooks);
    let mut controller = harness.controller(hooks.clone());
    let mut execution = harness.execution.take().unwrap();
    let wake = execution.wake();
    let control = harness.control.clone();
    let release = Release(hooks);
    let controller_support::Harness {
        scratch: _sources,
        spool,
        ..
    } = harness;
    drop(spool);
    let driver = std::thread::spawn(move || {
        let end = controller.drive_local(&mut execution);
        drop(controller);
        drop(execution);
        end
    });
    guard::wait(&receive, "held stage");
    control.store(true, Ordering::Release);
    wake.notify().unwrap();
    assert_eq!(
        guard::finishes("returning host", move || driver.join().unwrap()),
        Boundary::HandOver
    );
    // Once idle worker contexts have dropped, only the held stage and this
    // test own Hooks. A lingering worker cannot hide a missing stage lease.
    assert_eq!(
        guard::finishes("idle worker context release", move || {
            while weak_hooks.strong_count() != 2 {
                std::thread::yield_now();
            }
            2
        }),
        2
    );
    assert!(
        weak_started.strong_count() > 0,
        "spool lock released beside a held stage"
    );
    drop(release);
    assert_eq!(
        guard::finishes("stage lease release", move || {
            while weak_started.strong_count() != 0 {
                std::thread::yield_now();
            }
            0
        }),
        0
    );
}
