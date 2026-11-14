mod common;
mod controller_support;
#[allow(dead_code)]
mod directory_support;
mod publication_support;
mod stage_support;
use std::sync::{Arc, atomic::AtomicBool};
use unityrs_batch::{phase::Account, publish, spool::Host};

#[test]
fn pool_drives_share_admission_and_preserve_winner_indices() {
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 2, 2);
    let controller = harness.controller(Arc::new(controller_support::Observe::default()));
    let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
    for (concurrency, block) in [1, 2, 8]
        .into_iter()
        .flat_map(|c| [512, 3_072, 4_096, 5_003, 65_536].map(|b| (c, b)))
    {
        let scene = publication_support::Scene::new(&["a", "b", "c", "d"], false, block);
        std::fs::create_dir(scene.scratch.join("sink")).unwrap();
        let directory = directory_support::preflight(
            &scene.scratch.join("sink"),
            1,
            directory_support::fs(true),
        )
        .unwrap();
        let outcomes = scene.outcomes("out", &Host);
        let owned = Arc::new(publish::Owned {
            facts: unityrs_batch::sys::Shared::new(scene.prepared).unwrap(),
            done: scene.done,
            workspace: scene.workspace,
            handoff: scene.handoff,
            directory: Arc::new(directory),
            outcomes,
            account: Arc::new(Account::new(if concurrency == 1 {
                publish::WORKING_SET
            } else {
                64 << 20
            })),
            gate: publish::Gate::default(),
            fs: Arc::new(Host),
            control: Arc::new(AtomicBool::new(false)),
            hooks: Arc::new(publish::NoHooks),
            progress: None,
            spool_lease: None,
        });
        let report = publish::parallel(&owned, &execution, concurrency).unwrap();
        assert_eq!(report.written, 4);
        for index in 0..4 {
            assert_eq!(
                owned.outcomes.loose(index).unwrap(),
                unityrs_batch::outcomes::Code::Written
            );
        }
        assert_eq!(owned.account.snapshot().input.used, 0);
        assert!(owned.account.snapshot().input.peak <= owned.account.limit());
    }
    execution.quiesce().unwrap();
}

#[derive(Default)]
struct Overlap {
    arrivals: std::sync::atomic::AtomicUsize,
    lock: std::sync::Mutex<()>,
    changed: std::sync::Condvar,
}
impl publish::Hooks for Overlap {
    fn point(&self, point: publish::Point) {
        use std::sync::atomic::Ordering;
        use unityrs_testkit::guard::DEADLOCK_GUARD;
        if point != publish::Point::Transfer {
            return;
        }
        let guard = self.lock.lock().unwrap();
        let arrival = self.arrivals.fetch_add(1, Ordering::SeqCst);
        if arrival == 0 {
            let (_guard, waited) = self
                .changed
                .wait_timeout_while(guard, DEADLOCK_GUARD, |()| {
                    self.arrivals.load(Ordering::SeqCst) < 2
                })
                .unwrap();
            assert!(!waited.timed_out(), "publication did not overlap");
        } else {
            self.changed.notify_all();
        }
    }
}
#[test]
fn dispatcher_releases_its_lock_before_concurrent_transfers() {
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 2, 2);
    let controller = harness.controller(Arc::new(controller_support::Observe::default()));
    let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
    let scene = publication_support::Scene::new(&["a", "b", "c", "d"], false, 4096);
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    let directory =
        directory_support::preflight(&scene.scratch.join("sink"), 1, directory_support::fs(true))
            .unwrap();
    let outcomes = scene.outcomes("out", &Host);
    let overlap = Arc::new(Overlap::default());
    let owned = Arc::new(publish::Owned {
        facts: unityrs_batch::sys::Shared::new(scene.prepared).unwrap(),
        done: scene.done,
        workspace: scene.workspace,
        handoff: scene.handoff,
        directory: Arc::new(directory),
        outcomes,
        account: Arc::new(Account::new(64 << 20)),
        gate: publish::Gate::default(),
        fs: Arc::new(Host),
        control: Arc::new(AtomicBool::new(false)),
        hooks: overlap.clone(),
        progress: None,
        spool_lease: None,
    });
    let result = publish::parallel(&owned, &execution, 2).unwrap();
    assert_eq!(result.written, 4);
    assert_eq!(
        overlap.arrivals.load(std::sync::atomic::Ordering::SeqCst),
        4
    );
    execution.quiesce().unwrap();
}

static COUNT_BUFFERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BUFFERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REFUSE_BUFFER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct PoolAlloc;
impl unityrs_batch::alloc::Hook for PoolAlloc {
    fn refuses(&self, size: usize) -> bool {
        use std::sync::atomic::Ordering;
        if size == publish::BUFFER && COUNT_BUFFERS.load(Ordering::SeqCst) {
            BUFFERS.fetch_add(1, Ordering::SeqCst);
            return REFUSE_BUFFER.swap(false, Ordering::SeqCst);
        }
        false
    }
}
#[global_allocator]
static ALLOC: unityrs_batch::alloc::Counting<PoolAlloc> = unityrs_batch::alloc::Counting(PoolAlloc);
#[derive(Default)]
struct FailureControl {
    stopped: AtomicBool,
    failure: std::sync::Mutex<Option<unityrs_batch::job::RunFailure>>,
}
impl unityrs_batch::control::Control for FailureControl {
    fn stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Acquire)
    }
    fn request_stop(
        &self,
        failure: unityrs_batch::job::RunFailure,
        _: unityrs_batch::control::Culprit,
    ) {
        unityrs_batch::job::RunFailure::reduce(&mut self.failure.lock().unwrap(), failure);
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn pool_buffers_are_reused_and_pool_refusals_keep_the_host_memory_reason() {
    let output = unityrs_testkit::guard::finishes("concurrent buffer allocations", || {
        unityrs_testkit::child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pool_buffers_child", "--nocapture"])
                .env("UNITYRS_POOL_BUFFERS", "1"),
        )
        .unwrap()
    });
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn pool_buffers_child() {
    use std::sync::atomic::Ordering;
    if std::env::var_os("UNITYRS_POOL_BUFFERS").is_none() {
        return;
    }
    for refused in [false, true] {
        let mut harness = controller_support::Harness::new(
            &[("a", controller_support::clear(1), vec![])],
            &[],
            2,
            2,
        );
        let controller = harness.controller(Arc::new(controller_support::Observe::default()));
        let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
        let names: Vec<_> = (0..32).map(|n| format!("key-{n}")).collect();
        let keys: Vec<_> = names.iter().map(String::as_str).collect();
        let scene = publication_support::Scene::new(&keys, false, 4096);
        std::fs::create_dir(scene.scratch.join("sink")).unwrap();
        let directory = directory_support::preflight(
            &scene.scratch.join("sink"),
            1,
            directory_support::fs(true),
        )
        .unwrap();
        let outcomes = scene.outcomes("out", &Host);
        let control = Arc::new(FailureControl::default());
        let owned = Arc::new(publish::Owned {
            facts: unityrs_batch::sys::Shared::new(scene.prepared).unwrap(),
            done: scene.done,
            workspace: scene.workspace,
            handoff: scene.handoff,
            directory: Arc::new(directory),
            outcomes,
            account: Arc::new(Account::new(64 << 20)),
            gate: publish::Gate::default(),
            fs: Arc::new(Host),
            control: control.clone(),
            hooks: Arc::new(publish::NoHooks),
            progress: None,
            spool_lease: None,
        });
        BUFFERS.store(0, Ordering::SeqCst);
        REFUSE_BUFFER.store(refused, Ordering::SeqCst);
        COUNT_BUFFERS.store(true, Ordering::SeqCst);
        let result = publish::parallel(&owned, &execution, 2);
        execution.quiesce().unwrap();
        COUNT_BUFFERS.store(false, Ordering::SeqCst);
        if refused {
            assert!(result.is_err());
            assert_eq!(
                control.failure.lock().unwrap().unwrap().reason,
                unityrs_batch::attempt::ReasonCode::HostMemory
            );
        } else {
            assert_eq!(result.unwrap().written, 32);
            assert_eq!(BUFFERS.load(Ordering::SeqCst), 2);
        }
        assert_eq!(owned.account.snapshot().input.used, 0);
    }
}
