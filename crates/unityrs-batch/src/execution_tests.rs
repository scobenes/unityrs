use crate::{
    execution::Execution,
    phase1, spec,
    sys::{Shared, Slot},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn prepared(dc: u32) -> phase1::PreparedRun {
    let raw = serde_json::json!({
        "spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":dc,
        "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,
        "dep_cache_bytes":33_554_432,
        "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
        "inputs":[{"id":"a","path":"/in/a"}],"dependencies":[],
        "outputs":{"textures":{"key":"{input}/{name}.png"}},
        "sink":{"kind":"directory","root":"/out"}
    });
    phase1::prepare(spec::parse(serde_json::to_vec(&raw).unwrap()).unwrap()).unwrap()
}
/// One pool, `dc + 1` places: a further submission is refused, never
/// queued behind a held task, and a finished task frees its place.
#[test]
fn at_most_dc_plus_one_tasks_are_outstanding() {
    let () = unityrs_testkit::guard::finishes("pool capacity", || {
        for dc in [1, 2, 4] {
            let mut execution = Execution::prepare(&prepared(dc)).unwrap();
            let places = dc as usize + 1;
            let (release, held) = std::sync::mpsc::channel::<()>();
            let held = Arc::new(std::sync::Mutex::new(held));
            let started = Arc::new(AtomicUsize::new(0));
            let mut tasks = Vec::new();
            for _ in 0..places {
                let (held, started) = (held.clone(), started.clone());
                tasks.push(
                    execution
                        .spawn(move || {
                            started.fetch_add(1, Ordering::SeqCst);
                            let _ = held.lock().unwrap().recv();
                        })
                        .unwrap(),
                );
            }
            let refused = execution.spawn(|| ()).err().map(|e| e.kind());
            assert_eq!(refused, Some(std::io::ErrorKind::WouldBlock), "dc {dc}");
            assert_eq!(execution.pool_peak(), places);
            for _ in 0..places {
                release.send(()).unwrap();
            }
            while !tasks.iter().all(super::Task::is_finished) {
                execution.wait().unwrap();
            }
            assert_eq!(started.load(Ordering::SeqCst), places);
            for task in &tasks {
                execution.finish(task).unwrap();
            }
            // The places are free again.
            let again = execution.spawn(|| 7).unwrap();
            while !again.is_finished() {
                execution.wait().unwrap();
            }
            assert_eq!(execution.finish(&again), Ok(7));
            assert_eq!(execution.pool_peak(), places);
            execution.quiesce().unwrap();
        }
    });
}
/// The library Execution's drop never waits on a held pool task: its
/// runtime is shut down in the background, the task keeping its captures.
#[test]
fn dropping_the_library_execution_does_not_wait_for_a_held_task() {
    let () = unityrs_testkit::guard::finishes("background shutdown", || {
        let execution = Execution::prepare(&prepared(1)).unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (entered, started) = std::sync::mpsc::channel::<()>();
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = Dropped(drops.clone());
        let _task = execution
            .spawn(move || {
                let _capture = capture;
                entered.send(()).unwrap();
                let _ = held.recv();
            })
            .unwrap();
        started.recv().unwrap();
        let before = std::time::Instant::now();
        drop(execution);
        assert!(before.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(drops.load(Ordering::SeqCst), 0, "a held capture dropped");
        release.send(()).unwrap();
    });
}
#[test]
fn pool_wakes_only_after_the_result_is_available() {
    let () = unityrs_testkit::guard::finishes("pool result before wake", || {
        let prepared = prepared(1);
        let mut execution = Execution::prepare(&prepared).unwrap();
        for expected in 0..32 {
            let task = execution.spawn(move || expected).unwrap();
            execution.wait().unwrap();
            assert!(task.is_finished(), "wake preceded result");
            assert_eq!(execution.finish(&task).unwrap(), expected);
        }
        execution.quiesce().unwrap();
    });
}
struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn reserved_and_shared_storage_drop_the_value_exactly_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let slot = Slot::reserve().unwrap();
    let value = slot.write(Dropped(drops.clone()));
    let other = value.clone();
    drop(value);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    std::thread::spawn(move || drop(other)).join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(Shared::new(Dropped(drops.clone())).unwrap());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(Slot::<Dropped>::reserve().unwrap());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn startup_gate_releases_waiters_when_setup_returns_an_error() {
    let () = unityrs_testkit::guard::finishes("startup failure releases workers", || {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let (done, completed) = std::sync::mpsc::sync_channel(1);
        let worker;
        let result = {
            let gate = super::StartupGate::new();
            let wait = gate.0.clone();
            worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                super::StartupGate::wait(&wait);
                done.send(()).unwrap();
            });
            unityrs_testkit::guard::wait(&ready, "worker reached startup gate");
            // The same unconditional Drop runs on any spawn/readiness error.
            let fail_setup = || -> Result<(), ()> {
                let _gate = gate;
                Err(())
            };
            fail_setup()
        };
        assert_eq!(result, Err(()));
        unityrs_testkit::guard::wait(&completed, "worker released after setup failure");
        worker.join().unwrap();
    });
}
