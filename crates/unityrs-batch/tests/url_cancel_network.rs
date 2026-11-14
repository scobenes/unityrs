//! Canonical stop through real Service lookup and TLS waits in full staging.
#![cfg(feature = "test-hooks")]
mod http_support;
mod stage_support;
mod url_support;
use http_support::tls::{Handshake, Peer, Table};
use std::{
    future::{Future, poll_fn},
    io,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Poll, Waker},
    time::Duration,
};
use unityrs_batch::{
    http::{
        self, Payload, Service,
        permit::{OperationToken, PermitPool, RequestPermit},
        resolve::{Addresses, Resolver},
    },
    spool::{Host, OwnedFile},
    stage::{
        timers::{Context, RuntimeClock},
        url::{Stage, Transport},
    },
};
use url_support::Scene;

#[derive(Default)]
struct Barrier {
    entered: AtomicBool,
    waiter: Mutex<Option<Waker>>,
    gate: (Mutex<bool>, Condvar),
}
impl Barrier {
    fn enter(&self) {
        self.entered.store(true, Ordering::Release);
        if let Some(waker) = self.waiter.lock().unwrap().take() {
            waker.wake();
        }
    }
    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}
impl Resolver for Barrier {
    fn resolve(&self, _: &str, _: u16) -> io::Result<Addresses> {
        self.enter();
        let (released, _) = self
            .gate
            .1
            .wait_timeout_while(
                self.gate.0.lock().unwrap(),
                unityrs_testkit::guard::DEADLOCK_GUARD,
                |released| !*released,
            )
            .unwrap();
        assert!(*released, "lookup release guard");
        Ok(Addresses::default())
    }
}
struct Observed {
    service: Service,
    attempts: AtomicUsize,
}
impl Transport for Observed {
    type Response = http::Response<http::Stream>;
    async fn request(
        &self,
        permit: &mut RequestPermit,
        url: &url::Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: OperationToken,
    ) -> Result<Self::Response, http::Error> {
        self.attempts.fetch_add(1, Ordering::AcqRel);
        self.service
            .request_with_token(permit, url, request, timeout, token)
            .await
    }
}

#[test]
fn stop_during_held_lookup_and_tls_freezes_the_request_until_retirement() {
    let _serial = http_support::tls::lock();
    for tls in [false, true] {
        let mut scene = Scene::new(vec![]);
        scene.bundle["url"] = serde_json::json!(if tls {
            "https://localhost/x"
        } else {
            "http://localhost/x"
        });
        scene.bundle["size"] = serde_json::json!(3);
        scene.policy.attempts = 3;
        let barrier = Arc::new(Barrier::default());
        let peer = tls.then(|| {
            let barrier = barrier.clone();
            Peer::handshake_observed(Handshake::Stall, Arc::new(move || barrier.enter()))
        });
        let table = Arc::new(Table::default());
        let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let service = if let Some(peer) = &peer {
            table.service(runtime.handle().clone(), &[peer.address], true)
        } else {
            Service::new(runtime.handle().clone(), table.clone())
                .unwrap()
                .with_test_hooks(barrier.clone(), &[])
                .unwrap()
        };
        let transport = Observed {
            service,
            attempts: AtomicUsize::new(0),
        };
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
            clock: Arc::new(RuntimeClock::default()),
            control: scene.stop.clone(),
            cutoff: None,
        };
        {
            let mut future = std::pin::pin!(stage.run(
                0,
                validated.spec.bundle(0).unwrap(),
                scene.m,
                &scene.policy,
                &(),
                &transport,
                &mut permit,
                runtime.handle(),
                &cx
            ));
            runtime.block_on(async {
                tokio::time::timeout(
                    unityrs_testkit::guard::DEADLOCK_GUARD,
                    poll_fn(|task| {
                        *barrier.waiter.lock().unwrap() = Some(task.waker().clone());
                        assert!(
                            future.as_mut().poll(task).is_pending(),
                            "no received outcome before stop"
                        );
                        if barrier.entered.load(Ordering::Acquire) {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        }
                    }),
                )
                .await
                .expect("phase entry guard");
            });
            assert_eq!(transport.attempts.load(Ordering::Acquire), 1);
            assert_eq!(scene.counter.used(), 4096);
            assert_eq!(permits.snapshot().outstanding, usize::from(!tls));
            assert_eq!(table.added.load(Ordering::Acquire), usize::from(tls));
            assert_eq!(table.removed.load(Ordering::Acquire), 0);
            scene.stop.stopped.store(true, Ordering::Release);
            // Stop never turns a pending request into a received failure/facts.
            runtime.block_on(poll_fn(|task| {
                assert!(future.as_mut().poll(task).is_pending());
                Poll::Ready(())
            }));
            assert_eq!(transport.attempts.load(Ordering::Acquire), 1);
            assert_eq!(scene.counter.used(), 4096);
            assert_eq!(table.removed.load(Ordering::Acquire), 0);
            assert!(scene.stop.failures.lock().unwrap().is_empty());
        }
        retire(
            stage, permit, &permits, &scene, &table, runtime, &barrier, peer,
        );
    }
}
#[allow(clippy::too_many_arguments)]
fn retire(
    mut stage: Stage<http::Response<http::Stream>>,
    permit: RequestPermit,
    permits: &PermitPool,
    scene: &Scene,
    table: &Table,
    runtime: tokio::runtime::Runtime,
    barrier: &Barrier,
    peer: Option<Peer>,
) {
    let tls = peer.is_some();
    runtime.block_on(stage.quiesce()).unwrap();
    if !tls {
        // The resolver closure, unlike its dropped awaiter, still owns
        // its lookup token and charged storage until explicit release.
        assert_eq!(permits.snapshot().outstanding, 1);
        assert_eq!(
            permit.charged(),
            unityrs_batch::stage::url::STAGE_ENVELOPE + http::resolve::LOOKUP_ENVELOPE
        );
    }
    barrier.release();
    drop(runtime); // joins the now-released resolver, without wall sleeps
    assert_eq!(permits.snapshot().outstanding, 0);
    assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
    assert_eq!(table.removed.load(Ordering::Acquire), usize::from(tls));
    if let Some(peer) = peer {
        assert!(peer.finish().unwrap().reset);
    }
    let file = stage.file.as_mut().unwrap();
    assert_eq!(
        (file.length(), file.claimed(), file.is_sealed()),
        (0, 4096, false)
    );
    file.remove().unwrap();
    assert_eq!(scene.counter.used(), 0);
    drop(stage);
    assert_eq!(permit.charged(), 0);
    drop(permit);
    assert_eq!(permits.snapshot().occupied, 0);
}
