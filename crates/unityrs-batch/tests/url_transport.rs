//! B10a's loopback/TLS hook through the complete standalone stage.
#![cfg(feature = "test-hooks")]
mod http_support;
mod stage_support;
mod url_support;
use http_support::tls::{Peer, Table};
use serde_json::json;
use std::sync::Arc;
use unityrs_batch::{
    http::{Detail, permit::PermitPool},
    spool::{Counter, Host, OwnedFile},
    stage::{
        DataFailure, Failure,
        timers::{Context, RuntimeClock},
        url::Stage,
    },
};
use unityrs_testkit::scratch::Scratch;

#[test]
fn https_stages_through_service_and_keeps_tls_failures_typed() {
    let _serial = http_support::tls::lock();
    for roots in [true, false] {
        let peer = Peer::tls(
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec(),
            roots,
        );
        let table = Arc::new(Table::default());
        let scratch = Scratch::new("url-tls-stage");
        let validated = stage_support::from_bundles(
            &[
                json!({"id":"x","url":"https://localhost/?query_secret=hidden","headers":{"User-Agent":"stage-test"}}),
            ],
            &[],
        );
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let permits = PermitPool::new(1).unwrap();
        let mut permit = permits.acquire().unwrap();
        let file = OwnedFile::create(scratch.join("i0"), counter.clone(), Arc::new(Host)).unwrap();
        let mut stage = Stage::new(file, &permit).unwrap();
        let end = http_support::run(async {
            let pool = tokio::runtime::Handle::current();
            let service = table.service(pool.clone(), &[peer.address], roots);
            let cx = Context {
                clock: Arc::new(RuntimeClock::default()),
                control: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                cutoff: None,
            };
            stage
                .run(
                    0,
                    validated.spec.bundle(0).unwrap(),
                    4096,
                    &unityrs_batch::spec::model::Http {
                        attempts: 1,
                        ..Default::default()
                    },
                    &(),
                    &service,
                    &mut permit,
                    &pool,
                    &cx,
                )
                .await
        });
        assert!(!format!("{end:?}").contains("query_secret"));
        if roots {
            assert_eq!(end.failure, None);
            assert_eq!(
                std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
                b"abc"
            );
            let seen = peer.finish().unwrap();
            assert!(seen.reset);
            assert!(
                String::from_utf8(seen.request)
                    .unwrap()
                    .contains("user-agent: stage-test")
            );
        } else {
            assert_eq!(
                end.failure,
                Some(Failure::Data(DataFailure::DownloadFailed(Detail::Tls)))
            );
            assert!(peer.finish().is_err());
            assert_eq!(counter.used(), 0);
        }
        assert_eq!(permits.snapshot().outstanding, 0);
    }
}

struct NextAddress {
    calls: std::sync::atomic::AtomicUsize,
    addresses: std::sync::Mutex<std::collections::VecDeque<Option<std::net::SocketAddr>>>,
}
impl unityrs_batch::http::resolve::Resolver for NextAddress {
    fn resolve(&self, _: &str, _: u16) -> std::io::Result<unityrs_batch::http::resolve::Addresses> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.addresses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap()
            .map_or_else(
                || Err(std::io::ErrorKind::NotFound.into()),
                |address| Ok(unityrs_batch::http::resolve::Addresses::first([address])),
            )
    }
}

#[test]
fn service_connect_and_tls_faults_cross_staging_retry_reset_and_fact_cleanup() {
    let _serial = http_support::tls::lock();
    for kind in [
        "resolve",
        "refused",
        "untrusted",
        "wrong-name",
        "expired",
        "protocol",
        "eof",
        "reset",
        "stall",
        "idle",
    ] {
        for attempts in [1, 2] {
            service_fault(kind, attempts);
        }
    }
}
fn service_fault(kind: &str, attempts: u32) {
    use std::sync::atomic::Ordering;
    let (peers, resolver, notice) = fault_endpoints(kind, attempts);
    let table = Arc::new(Table::default());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let roots = if kind == "untrusted" {
        Vec::new()
    } else {
        vec![rustls::pki_types::CertificateDer::from(
            http_support::tls::ROOT.to_vec(),
        )]
    };
    let service = unityrs_batch::http::Service::new(runtime.handle().clone(), table.clone())
        .unwrap()
        .with_test_hooks(resolver.clone(), &roots)
        .unwrap();
    let mut scene = url_support::Scene::new(vec![]);
    scene.bundle["url"] = json!(if matches!(kind, "resolve" | "refused") {
        "http://localhost/x"
    } else {
        "https://localhost/x"
    });
    scene.bundle["size"] = json!(3);
    let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
    let permits = PermitPool::new(1).unwrap();
    let mut permit = permits.acquire().unwrap();
    let disk = url_support::disk::Disk::new(scene.counter.clone(), scene.time.clone());
    let file = OwnedFile::create(
        scene.scratch.join("i0"),
        scene.counter.clone(),
        Arc::new(url_support::disk::Fs(disk.clone())),
    )
    .unwrap();
    let mut stage = Stage::new(file, &permit).unwrap();
    let cx = Context {
        clock: scene.time.clone(),
        control: scene.stop.clone(),
        cutoff: None,
    };
    let policy = unityrs_batch::spec::model::Http {
        attempts,
        backoff_initial_ms: 0,
        backoff_max_ms: 0,
        connect_timeout_ms: match kind {
            "idle" => 3000,
            "stall" => 100,
            _ => 5000,
        },
        idle_timeout_ms: 1000,
        ..Default::default()
    };
    let end = runtime.block_on(advance_at_handshake(&notice, &scene.time, async {
        tokio::time::timeout(
            unityrs_testkit::guard::DEADLOCK_GUARD,
            stage.run(
                0,
                validated.spec.bundle(0).unwrap(),
                scene.m,
                &policy,
                &(),
                &service,
                &mut permit,
                runtime.handle(),
                &cx,
            ),
        )
        .await
        .expect("service fault guard")
    }));
    assert_fault_outcome(kind, attempts, &end, &scene, &resolver, &disk);
    assert!(!stage.file.as_ref().unwrap().path().exists());
    assert_eq!(permits.snapshot().outstanding, 0);
    assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
    let sockets = if kind == "resolve" {
        0
    } else {
        attempts as usize
    };
    assert_eq!(table.added.load(Ordering::Acquire), sockets);
    assert_eq!(table.removed.load(Ordering::Acquire), sockets);
    for peer in peers {
        let result = peer.finish();
        if matches!(kind, "untrusted" | "wrong-name" | "expired") {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok(), "{kind}: {result:?}");
        }
    }
}

fn assert_fault_outcome(
    kind: &str,
    attempts: u32,
    end: &unityrs_batch::stage::StageEnd,
    scene: &url_support::Scene,
    resolver: &NextAddress,
    disk: &url_support::disk::Disk,
) {
    use std::sync::atomic::Ordering;
    let detail = match kind {
        "untrusted" | "wrong-name" | "expired" | "protocol" => Detail::Tls,
        "idle" => Detail::Timeout,
        _ => Detail::Connect,
    };
    assert_eq!(end.failure, url_support::download(detail), "{kind}");
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    assert_eq!(
        resolver.calls.load(Ordering::Acquire),
        attempts as usize,
        "{kind}"
    );
    assert_eq!(scene.counter.used(), 0);
    assert!(scene.stop.failures.lock().unwrap().is_empty());
    let events = disk.events.lock().unwrap();
    let resets: Vec<_> = events
        .iter()
        .filter(|e| e.operation == "truncate")
        .collect();
    assert_eq!(resets.len(), attempts as usize - 1);
    assert!(resets.iter().all(|e| e.claim == 4096 && e.length == 0));
    assert!(
        events
            .iter()
            .all(|e| e.operation != "write" && e.operation != "seal")
    );
}

fn fault_endpoints(kind: &str, attempts: u32) -> (Vec<Peer>, Arc<NextAddress>, Arc<Notice>) {
    use http_support::tls::Handshake;
    let notice = Arc::new(Notice::default());
    let mut peers = Vec::new();
    let mut addresses = std::collections::VecDeque::new();
    let refused = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let refused_address = refused.local_addr().unwrap();
    drop(refused);
    for _ in 0..attempts {
        let peer = match kind {
            "resolve" | "refused" => None,
            "wrong-name" => Some(Peer::certificate(Vec::new(), http_support::tls::WRONG_HOST)),
            "expired" => Some(Peer::certificate(Vec::new(), http_support::tls::EXPIRED)),
            "untrusted" => Some(Peer::tls(Vec::new(), false)),
            "protocol" => Some(Peer::handshake(Handshake::Protocol)),
            "eof" => Some(Peer::handshake(Handshake::Eof)),
            "reset" => Some(Peer::handshake(Handshake::Reset)),
            "idle" => {
                let notice = notice.clone();
                Some(Peer::handshake_observed(
                    Handshake::Stall,
                    Arc::new(move || {
                        notice
                            .entered
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        if let Some(waker) = notice.waiter.lock().unwrap().take() {
                            waker.wake();
                        }
                    }),
                ))
            }
            _ => Some(Peer::handshake(Handshake::Stall)),
        };
        addresses.push_back(
            peer.as_ref()
                .map(|p| p.address)
                .or_else(|| (kind == "refused").then_some(refused_address)),
        );
        if let Some(peer) = peer {
            peers.push(peer);
        }
    }
    let resolver = Arc::new(NextAddress {
        calls: std::sync::atomic::AtomicUsize::new(0),
        addresses: std::sync::Mutex::new(addresses),
    });
    (peers, resolver, notice)
}

#[derive(Default)]
struct Notice {
    entered: std::sync::atomic::AtomicUsize,
    waiter: std::sync::Mutex<Option<std::task::Waker>>,
}
async fn advance_at_handshake<F: std::future::Future>(
    notice: &Notice,
    time: &url_support::Time,
    future: F,
) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut delivered = 0;
    std::future::poll_fn(|task| {
        *notice.waiter.lock().unwrap() = Some(task.waker().clone());
        let result = std::future::Future::poll(future.as_mut(), task);
        let entered = notice.entered.load(std::sync::atomic::Ordering::Acquire);
        if entered > delivered {
            // Real TLS has consumed ClientHello. Expire the injected body
            // timer before the still-pending connection timer, without a
            // scheduler-speed assumption or a wall-clock body timeout.
            delivered = entered;
            time.at(1000 * u64::try_from(delivered).unwrap());
            task.waker().wake_by_ref();
        }
        result
    })
    .await
}
