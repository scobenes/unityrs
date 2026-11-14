//! The library Execution's threads: `threads` workers and `dc + 1` pool
//! threads, all started by its constructor, and all gone once it is dropped
//! (its runtime shut down in the background). Alone in its binary, so
//! no other test's threads move the baseline.
#![cfg(target_os = "linux")]
use std::time::{Duration, Instant};
use unityrs_batch::{execution::Execution, phase1, spec};

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}
fn settle(expected: usize) -> usize {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let now = threads();
        if now == expected || Instant::now() > until {
            return now;
        }
        std::thread::yield_now();
    }
}
#[test]
fn the_library_execution_returns_to_its_thread_baseline_on_drop() {
    for (workers, dc) in [(1_u32, 1_u32), (2, 3)] {
        let raw = serde_json::json!({
            "spec_version":1,"run_id":"r","attempt":1,"threads":workers,
            "download_concurrency":dc,
            "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,
            "dep_cache_bytes":33_554_432,
            "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
            "inputs":[{"id":"a","path":"/in/a"}],"dependencies":[],
            "outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/out"}
        });
        let prepared =
            phase1::prepare(spec::parse(serde_json::to_vec(&raw).unwrap()).unwrap()).unwrap();
        let baseline = threads();
        let execution = Execution::prepare(&prepared).unwrap();
        // Every thread exists once the constructor returns: none is spawned
        // after it, so none can be refused after the head.
        assert_eq!(threads(), baseline + (workers + dc + 1) as usize);
        drop(execution);
        assert_eq!(settle(baseline), baseline, "workers {workers} dc {dc}");
    }
}
