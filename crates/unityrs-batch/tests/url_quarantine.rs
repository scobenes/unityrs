//! A lookup that outlives its request's last try keeps its permit until it
//! returns; the staging pass waiting for a permit is woken by that return.
//! Before the pool's waiter, a quarantined lookup's return woke no one, and
//! a further bundle waited forever at `download_concurrency` 1 and 2.
#![cfg(feature = "test-hooks")]
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use serde_json::json;
use std::{
    fs, io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use unityrs_batch::{
    http::resolve::{Addresses, Resolver},
    phase1,
    progress::{Snapshot, live::Service},
    run::{self, Hooks, Network, Options},
};
use unityrs_testkit::{guard, scratch::Scratch};
use url_run_support::Origin;
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> io::Result<u64> {
        Ok(u64::MAX)
    }
}
/// `localhost` resolves slowly, past the connect timeout; anything else at
/// once. Both to the origin.
struct Slow(SocketAddr);
impl Resolver for Slow {
    fn resolve(&self, host: &str, _: u16) -> io::Result<Addresses> {
        if host == "localhost" {
            std::thread::sleep(Duration::from_millis(1_500));
        }
        Ok(Addresses::first([self.0]))
    }
}
/// `slow` bundles go to `localhost`, the rest to the origin's address; one
/// try each, a 300 ms connect timeout.
fn run(concurrency: u64, slow: usize) {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("url-quarantine");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    // A further, unused copy of the dependency waits for a permit.
    let mut copy = spec["dependencies"][0].clone();
    copy["id"] = json!("copy");
    spec["dependencies"].as_array_mut().unwrap().push(copy);
    let origin = Origin::start();
    url_run_support::onto(&origin, &mut spec, |_| true);
    let mut n = 0;
    for list in ["inputs", "dependencies"] {
        for bundle in spec[list].as_array_mut().unwrap() {
            if n < slow {
                let url = bundle["url"]
                    .as_str()
                    .unwrap()
                    .replace("127.0.0.1", "localhost");
                bundle["url"] = json!(url);
            }
            n += 1;
        }
    }
    spec["threads"] = json!(1);
    spec["download_concurrency"] = json!(concurrency);
    spec["http"] = json!({"attempts": 1, "connect_timeout_ms": 300});
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let options = Options {
        network: Some(Network {
            hooks: Some(Hooks {
                resolver: Arc::new(Slow(origin.address)),
                extra_roots: Vec::new(),
            }),
            ..Network::system()
        }),
        ..Options::default()
    };
    let started = Instant::now();
    let sender = service.sender();
    let end = guard::finishes("quarantined lookup's permit", move || {
        run::drive_with(
            prepared,
            0,
            &sender,
            (&Space, Arc::new(publication_support::Portable)),
            &options,
        )
        .unwrap()
    });
    // The slow bundles timed out (the input's, so a failed view: 4); the
    // copy waited for a returned lookup's permit, then staged.
    assert_eq!(end.code.value(), 4, "{end:?}");
    assert!(started.elapsed() >= Duration::from_millis(1_400));
    assert_eq!(origin.requests("/b2"), 1);
    assert_eq!(origin.requests("/b0"), 0);
}
#[test]
fn a_quarantined_lookup_frees_the_only_permit() {
    run(1, 1);
}
#[test]
fn two_quarantined_lookups_free_both_permits() {
    run(2, 2);
}
