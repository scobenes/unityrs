//! A thousand URL bundles at `download_concurrency` 256: every one staged
//! once, never more connections open than permits, each closed by a reset.
//! Count all threads, including the peer, conservatively: fixture storage
//! stays live in the baseline and the run's new peer observations count too.
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use serde_json::json;
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    bound::{self, PROVISIONAL},
    controller,
    http::permit::{NETWORK_ALLOWANCE, Snapshot as Permits},
    phase1,
    progress::{Snapshot, live::Service},
    run::{self, Network, Options},
};
use unityrs_testkit::scratch::Scratch;
use url_run_support::{Close, Origin};
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
struct Heap;
impl Hook for Heap {
    fn grew(&self, size: usize, _: bool) {
        let cost = unityrs_core::limits::heap_cost(size as u64);
        let live = LIVE.fetch_add(cost, Ordering::AcqRel) + cost;
        PEAK.fetch_max(live, Ordering::AcqRel);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(
            unityrs_core::limits::heap_cost(size as u64),
            Ordering::AcqRel,
        );
    }
}
#[global_allocator]
static ALLOCATOR: Counting<Heap> = Counting(Heap);
#[derive(Default)]
struct Idle {
    calls: AtomicUsize,
    heap_peak: AtomicU64,
}
impl unityrs_export::ngui::Hooks for Idle {}
impl controller::Hooks for Idle {
    fn network_idle(&self, permits: Permits) {
        // Occupancy includes requests, outstanding operations and retained
        // charges: zero means all 256 slots are available again.
        assert_eq!(permits, Permits::default());
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.heap_peak
            .store(PEAK.load(Ordering::Acquire), Ordering::Release);
    }
}
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
const BUNDLES: usize = 1_000;
const CONCURRENCY: u64 = 256;
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn a_thousand_urls_stage_within_the_permits() {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("url-scale");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    // Unused copies of the dependency, each its own URL, beside the case's
    // own input and dependency: the case's outputs stand unchanged.
    let dependency = spec["dependencies"][0].clone();
    let dependencies = spec["dependencies"].as_array_mut().unwrap();
    for n in 1..BUNDLES - 1 {
        let mut copy = dependency.clone();
        copy["id"] = json!(format!("copy-{n}"));
        dependencies.push(copy);
    }
    spec["threads"] = json!(8);
    spec["download_concurrency"] = json!(CONCURRENCY);
    spec["spool"]["size_bytes"] = json!(1u64 << 32);
    let origin = Origin::start();
    assert_eq!(
        url_run_support::onto(&origin, &mut spec, |_| true).len(),
        BUNDLES
    );
    let (prepared, measured) = unityrs_batch::alloc::measured(|| {
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap()
    });
    let run_bound = bound::memory_bound(&prepared.spec, measured, 0, &PROVISIONAL);
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let idle = Arc::new(Idle::default());
    let baseline = LIVE.load(Ordering::Acquire);
    // The count has been active since process startup. No late frees of
    // baseline allocations can subtract from an uncounted window.
    PEAK.store(baseline, Ordering::Release);
    let end = run::drive_with(
        prepared,
        run_bound.total(),
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &Options {
            network: Some(Network::system()),
            controller_hooks: idle.clone(),
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(end.code.value(), 0);
    assert_eq!(idle.calls.load(Ordering::Acquire), 1);
    let staging_peak = idle
        .heap_peak
        .load(Ordering::Acquire)
        .saturating_sub(baseline);
    let staging_bound = run_bound.c_base + CONCURRENCY * NETWORK_ALLOWANCE;
    assert!(
        staging_peak <= staging_bound,
        "staging heap {staging_peak} > {staging_bound}"
    );
    let full_peak = PEAK.load(Ordering::Acquire).saturating_sub(baseline);
    assert!(
        full_peak <= run_bound.total(),
        "run heap {full_peak} > {}",
        run_bound.total()
    );
    println!(
        "URL_RUN_SCALE bundles={BUNDLES} dc={CONCURRENCY} staging_heap={staging_peak} staging_bound={staging_bound} run_heap={full_peak} occupied=0 outstanding=0"
    );
    let snapshot = service.sender().snapshot().unwrap();
    assert_eq!(snapshot.transfers.stages_done, BUNDLES as u64);
    assert_eq!(snapshot.transfers.stages_active, 0);
    assert_eq!(origin.total(), BUNDLES);
    assert!(origin.most_open() <= 256, "{}", origin.most_open());
    assert!(origin.wait_closed().iter().all(|c| *c == Close::Reset));
    let outputs: Vec<_> = case["outputs"].as_object().unwrap().keys().collect();
    for key in outputs {
        assert!(scratch.join("sink").join(key).is_file(), "{key}");
    }
}
