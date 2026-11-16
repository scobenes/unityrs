//! Startup must initialize retained Tokio locks before the running head.
mod common;
use std::cell::Cell;
use unityrs_batch::{
    alloc::{Counting, Hook},
    execution::Execution,
    lifetime::Runtimes,
};

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static SEEN: Cell<usize> = const { Cell::new(0) };
}
struct Count;
impl Hook for Count {
    fn refuses(&self, _: usize) -> bool {
        if ACTIVE.with(|a| a.replace(false)) {
            let trace = std::backtrace::Backtrace::force_capture().to_string();
            // Channel teardown and the shutdown thread's cached parker are
            // separate owners. This regression guards runtime-retained locks.
            if [
                "tokio::runtime::scheduler::",
                "tokio::runtime::io::",
                "tokio::runtime::time::",
            ]
            .iter()
            .any(|owner| trace.contains(&format!("tokio::loom::std::mutex::Mutex<{owner}")))
                || trace.contains("tokio::loom::std::mutex::Mutex<tokio::util::linked_list::")
            {
                SEEN.with(|n| n.set(n.get() + 1));
                eprintln!("retained runtime lock allocation: {trace}");
            }
            ACTIVE.with(|a| a.set(true));
        }
        false
    }
}
#[global_allocator]
static ALLOC: Counting<Count> = Counting(Count);

fn without_runtime_lock_allocations(f: impl FnOnce()) {
    SEEN.with(|n| n.set(0));
    ACTIVE.with(|a| a.set(true));
    f();
    ACTIVE.with(|a| a.set(false));
    assert_eq!(
        SEEN.with(Cell::get),
        0,
        "first shutdown allocated after startup"
    );
}

#[test]
fn process_runtimes_prime_retained_locks_before_the_first_shutdown() {
    let () = unityrs_testkit::guard::finishes("prepared runtime shutdown", || {
        let runtimes = Runtimes::prepare(1).unwrap();
        runtimes.prewarm().unwrap();
        without_runtime_lock_allocations(|| drop(runtimes));
    });
}

#[test]
fn library_runtime_primes_retained_locks_before_the_first_shutdown() {
    let () = unityrs_testkit::guard::finishes("prepared execution shutdown", || {
        let prepared = unityrs_batch::phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&common::base()).unwrap()).unwrap(),
        )
        .unwrap();
        let execution = Execution::prepare(&prepared).unwrap();
        without_runtime_lock_allocations(|| drop(execution));
    });
}

#[test]
fn startup_priming_preserves_preparation_from_an_async_host() {
    let () = unityrs_testkit::guard::finishes("startup from async host", || {
        let host = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let runtimes = Runtimes::prepare(1).unwrap();
        host.block_on(async { runtimes.prewarm().unwrap() });
        let prepared = unityrs_batch::phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&common::base()).unwrap()).unwrap(),
        )
        .unwrap();
        // Keep shutdown in the async context too: Execution's drop must
        // still use background shutdown and never join its pool threads.
        host.block_on(async { drop(Execution::prepare(&prepared).unwrap()) });
    });
}

#[test]
fn concurrent_runtime_preparation_initializes_every_task_shard() {
    let () = unityrs_testkit::guard::finishes("concurrent runtime preparation", || {
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut failures = Vec::new();
                        for _ in 0..8 {
                            let runtimes = Runtimes::prepare(1).unwrap();
                            barrier.wait();
                            let result = runtimes.prewarm();
                            // Every worker reaches the next barrier even if
                            // startup failed, so failures cannot strand peers.
                            barrier.wait();
                            if let Err(error) = result {
                                failures.push(error);
                            }
                        }
                        failures
                    })
                })
                .collect();
            for worker in workers {
                assert!(worker.join().unwrap().is_empty());
            }
        });
    });
}
