//! A thousand URL bundles at `download_concurrency` 256: every one staged
//! once, never more connections open than permits, each closed by a reset.
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use serde_json::json;
use std::{fs, sync::Arc};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    run::{self, Network, Options},
};
use unityrs_testkit::scratch::Scratch;
use url_run_support::{Close, Origin};
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
    spec["download_concurrency"] = json!(256);
    spec["spool"]["size_bytes"] = json!(1u64 << 32);
    let origin = Origin::start();
    assert_eq!(
        url_run_support::onto(&origin, &mut spec, |_| true).len(),
        BUNDLES
    );
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let end = run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &Options {
            network: Some(Network::system()),
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(end.code.value(), 0);
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
