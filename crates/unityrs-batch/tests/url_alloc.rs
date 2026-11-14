//! Count complete staging, pool writes included. Scripted peer bytes and the
//! validated spec are prepared outside the measured network window.
#[path = "http_support/count.rs"]
mod count;
mod http_support;
mod stage_support;
mod url_support;
use serde_json::json;
use std::sync::Arc;
use unityrs_batch::{
    alloc::Counting,
    http::permit::{NETWORK_ALLOWANCE, PermitPool},
    spool::{Host, OwnedFile},
    stage::{timers::Context, url::Stage},
};
use url_support::{Reply, Scene};
#[global_allocator]
static ALLOCATOR: Counting<count::Network> = Counting(count::Network);

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's scale job does"
)]
fn streamed_body_redirect_and_maximal_headers_fit_four_mib() {
    let _serial = count::lock();
    for scene_id in ["stream", "near-cap", "request", "redirect"] {
        let replies = match scene_id {
            "stream" => vec![Reply::ok(&vec![b'x'; 64 * 1024 * 1024])],
            "near-cap" => vec![Reply::raw(http_support::combined_response())],
            "redirect" => vec![
                Reply::status(302, &format!("Location: /{}\r\n", "a".repeat(16_340))),
                Reply::ok(b"ok"),
            ],
            _ => vec![Reply::ok(b"ok")],
        };
        // Keep uncounted peer backing alive until after finish: freeing it
        // inside the window would incorrectly subtract from engine live bytes.
        let backing = replies.iter().map(|r| r.raw.clone()).collect::<Vec<_>>();
        let mut scene = Scene::new(replies);
        scene.m = 128 * 1024 * 1024;
        if scene_id == "request" {
            let mut fields = serde_json::Map::new();
            for index in 0..32 {
                fields.insert(format!("x-{index}"), json!({"env":"TOKEN"}));
            }
            scene.bundle["headers"] = fields.into();
        }
        let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
        let values = Values("a".repeat(16_384));
        count::start();
        {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .on_thread_start(count::mark)
                .max_blocking_threads(2)
                .build()
                .unwrap();
            let permits = PermitPool::new(1).unwrap();
            let mut permit = permits.acquire().unwrap();
            let file = OwnedFile::create(
                scene.scratch.join("i0"),
                scene.counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let mut stage = Stage::new(file, &permit).unwrap();
            let cx = Context {
                clock: scene.time.clone(),
                control: scene.stop.clone(),
                cutoff: None,
            };
            let end = runtime.block_on(scene.time.drive(
                stage.run(
                    0,
                    validated.spec.bundle(0).unwrap(),
                    scene.m,
                    &scene.policy,
                    &values,
                    &scene.peer,
                    &mut permit,
                    runtime.handle(),
                    &cx,
                ),
                || permits.snapshot().outstanding,
            ));
            assert_eq!(end.failure, None);
            assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
            stage.file.as_mut().unwrap().remove().unwrap();
            drop(stage);
            assert_eq!(permit.charged(), 0);
            assert_eq!(permits.snapshot().outstanding, 0);
            drop(permit);
            assert_eq!(permits.snapshot().occupied, 0);
        }
        let measured = count::finish();
        drop(backing);
        assert!(
            measured.peak <= NETWORK_ALLOWANCE,
            "{scene_id}: {measured:?}"
        );
        assert_eq!(scene.counter.used(), 0);
        println!(
            "URL_MEMORY scene={scene_id} peak={} allowance={NETWORK_ALLOWANCE}",
            measured.peak
        );
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's scale job does"
)]
fn sequential_hosts_release_network_storage_and_stage_facts_fit_phase_one() {
    let _serial = count::lock();
    let mut bundles = Vec::new();
    for index in 0..1000 {
        bundles.push(json!({"id":format!("b{index}"),"url":format!("http://h{index}.invalid/x")}));
    }
    let validated = stage_support::from_bundles(&bundles, &[]);
    // Fixed typed HTTP detail fits the existing enum's u64-aligned slot.
    // These rows are allocated inside phase1::prepare's measured C_spec.
    let (prepared, figures) =
        unityrs_batch::alloc::measured(|| unityrs_batch::phase1::prepare(validated));
    let prepared = prepared.unwrap();
    assert_eq!(prepared.bundles.len(), 1000);
    println!(
        "URL_PHASE_ONE bundles=1000 stage_end={} bundle_state={} kept={} peak={}",
        std::mem::size_of::<unityrs_batch::stage::StageEnd>(),
        std::mem::size_of::<unityrs_batch::phase1::BundleState>(),
        figures.kept,
        figures.peak
    );
    let replies = vec![Reply::ok(b"ok"); 1000];
    let backing = replies[0].raw.clone();
    let scene = Scene::new(replies);
    count::start();
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .on_thread_start(count::mark)
            .build()
            .unwrap();
        let permits = PermitPool::new(1).unwrap();
        let mut permit = permits.acquire().unwrap();
        for index in 0..1000 {
            let file = OwnedFile::create(
                scene.scratch.join("i0"),
                scene.counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let mut stage = Stage::new(file, &permit).unwrap();
            let cx = Context {
                clock: scene.time.clone(),
                control: scene.stop.clone(),
                cutoff: None,
            };
            let end = runtime.block_on(scene.time.drive(
                stage.run(
                    index,
                    prepared.spec.bundle(index as usize).unwrap(),
                    scene.m,
                    &scene.policy,
                    &(),
                    &scene.peer,
                    &mut permit,
                    runtime.handle(),
                    &cx,
                ),
                || permits.snapshot().outstanding,
            ));
            assert_eq!(end.failure, None);
            stage.file.as_mut().unwrap().remove().unwrap();
            drop(stage);
            assert_eq!(permit.charged(), 0);
            assert_eq!(scene.counter.used(), 0);
            // Peer observations belong to the test; never retain a history in
            // the measured engine window for the many-host scene.
            scene.peer.seen.lock().unwrap().clear();
        }
    }
    let measured = count::finish();
    drop(backing);
    assert!(measured.peak <= NETWORK_ALLOWANCE, "{measured:?}");
    println!(
        "URL_MEMORY scene=1000-hosts peak={} allowance={NETWORK_ALLOWANCE}",
        measured.peak
    );
}

struct Values(String);
impl unityrs_batch::stage::headers::Values for Values {
    fn get(&self, _: &str) -> Option<&str> {
        Some(&self.0)
    }
}

#[cfg(feature = "test-hooks")]
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's scale job does"
)]
fn maximal_staging_request_and_strongest_tls_with_ocsp_fit_both_envelopes() {
    use unityrs_batch::{http::LIBRARY_ENVELOPE, stage::timers::RuntimeClock};
    let _serial = count::lock();
    let peer = http_support::tls::Peer::with_aligned_config(
        http_support::combined_response(),
        http_support::stress::config("combined-minimal"),
    );
    let scratch = unityrs_testkit::scratch::Scratch::new("url-combined-memory");
    let mut fields = serde_json::Map::new();
    for index in 0..32 {
        fields.insert(
            format!("x-{index:02}{}", "a".repeat(2044)),
            json!({"env":"TOKEN"}),
        );
    }
    let bundle = json!({
        "id":"x",
        "url":format!("https://localhost/{}", "a".repeat(16_384 - 18)),
        "headers":fields,
    });
    let validated = stage_support::from_bundles(&[bundle], &[]);
    let values = Values("a".repeat(16_384));
    let counter = unityrs_batch::spool::Counter::new(1024 * 1024, 0, 4096).unwrap();
    count::start();
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .on_thread_start(count::mark)
            .build()
            .unwrap();
        let table = Arc::new(http_support::tls::Table::default());
        let service = table.service(runtime.handle().clone(), &[peer.address], true);
        let permits = PermitPool::new(1).unwrap();
        let mut permit = permits.acquire().unwrap();
        let file = OwnedFile::create(scratch.join("i0"), counter.clone(), Arc::new(Host)).unwrap();
        let mut stage = Stage::new(file, &permit).unwrap();
        let cx = Context {
            clock: Arc::new(RuntimeClock::default()),
            control: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cutoff: None,
        };
        let end = runtime.block_on(async {
            tokio::time::timeout(
                unityrs_testkit::guard::DEADLOCK_GUARD,
                stage.run(
                    0,
                    validated.spec.bundle(0).unwrap(),
                    1024 * 1024,
                    &unityrs_batch::spec::model::Http {
                        attempts: 1,
                        ..Default::default()
                    },
                    &values,
                    &service,
                    &mut permit,
                    runtime.handle(),
                    &cx,
                ),
            )
            .await
            .expect("combined staging guard")
        });
        assert_eq!(end.failure, None);
        assert_eq!(end.facts.unwrap().size, 131_072);
        assert_eq!(end.claimed_bytes, 131_072);
        assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
        stage.file.as_mut().unwrap().remove().unwrap();
        drop(stage);
        assert_eq!(permit.charged(), 0);
        assert_eq!(permits.snapshot().outstanding, 0);
        drop(permit);
        assert_eq!(permits.snapshot().occupied, 0);
        assert_eq!(table.added.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(table.removed.load(std::sync::atomic::Ordering::Acquire), 1);
    }
    let measured = count::finish();
    let library = count::library_peak();
    println!(
        "URL_COMBINED_MEMORY peak={} library_peak={library} library_envelope={LIBRARY_ENVELOPE} allowance={NETWORK_ALLOWANCE} headers=32 name_bytes=65536 value_bytes=16384 url_bytes=16384 tls=combined-minimal ocsp_bytes=65400",
        measured.peak
    );
    assert!(measured.peak <= NETWORK_ALLOWANCE, "{measured:?}");
    // No caller-held pieces or stage storage is subtracted: the full staging
    // peak must fit even this stricter library-only comparison.
    assert!(
        library <= LIBRARY_ENVELOPE,
        "library={library} envelope={LIBRARY_ENVELOPE}"
    );
    assert_eq!(counter.used(), 0);
    let seen = peer.finish().unwrap();
    assert!(seen.reset);
    assert!(seen.request.len() > 32 * 16_384 + 16_000);
}
