//! Empty DATA is neither persistence work nor body-timer credit.
#![cfg(feature = "test-hooks")]
mod http_support;
mod stage_support;
mod url_support;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use unityrs_batch::{
    http::{
        Data, Error, Payload,
        permit::{OperationToken, PermitPool, RequestPermit},
    },
    spool::OwnedFile,
    stage::{
        timers::{Clock, Context},
        url::{Body, Stage, Transport},
    },
};
use url_support::{Scene, Time};
struct EmptyPeer(Arc<Time>, bool, Arc<std::sync::atomic::AtomicUsize>);
struct EmptyBody {
    time: Arc<Time>,
    headers: hyper::HeaderMap,
    data: VecDeque<Data>,
    late: bool,
}
impl Body for EmptyBody {
    fn status(&self) -> u16 {
        200
    }
    fn headers(&self) -> &hyper::HeaderMap {
        &self.headers
    }
    async fn data(
        &mut self,
        credit: &mut impl FnMut(usize) -> Result<(), Error>,
    ) -> Result<Option<Data>, Error> {
        self.time
            .sleep_until(Duration::from_millis(if self.data.len() == 2 {
                999
            } else if self.late {
                1998
            } else {
                999
            }))
            .await;
        let data = self.data.pop_front();
        if let Some(data) = &data {
            credit(data.as_slice().len())?;
        }
        Ok(data)
    }
}
impl Transport for EmptyPeer {
    type Response = EmptyBody;
    fn request(
        &self,
        permit: &mut RequestPermit,
        _: &url::Url,
        _: hyper::Request<Payload>,
        _: Duration,
        token: OperationToken,
    ) -> impl std::future::Future<Output = Result<EmptyBody, Error>> {
        drop(token);
        self.2.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let result = Data::with_test_bytes(permit, b"").and_then(|first| {
            Data::with_test_bytes(permit, b"").map(|second| EmptyBody {
                time: self.0.clone(),
                headers: hyper::HeaderMap::new(),
                data: [first, second].into(),
                late: self.1,
            })
        });
        std::future::ready(result)
    }
}
#[test]
fn empty_data_never_writes_claims_or_refreshes_idle() {
    for late in [false, true] {
        let mut scene = Scene::new(vec![]);
        scene.policy.idle_timeout_ms = 1000;
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
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = EmptyPeer(scene.time.clone(), late, attempts.clone());
        let cx = Context {
            clock: scene.time.clone(),
            control: scene.stop.clone(),
            cutoff: None,
        };
        if !late {
            scene.time.at(999);
        }
        let end = http_support::run(scene.time.drive(
            async {
                let pool = tokio::runtime::Handle::current();
                let mut future = std::pin::pin!(stage.run(
                    0,
                    validated.spec.bundle(0).unwrap(),
                    0,
                    &scene.policy,
                    &(),
                    &transport,
                    &mut permit,
                    &pool,
                    &cx,
                ));
                std::future::poll_fn(|task| {
                    let result = std::future::Future::poll(future.as_mut(), task);
                    if !late {
                        assert!(result.is_ready(), "empty DATA must not spawn a write");
                    }
                    result
                })
                .await
            },
            || permits.snapshot().outstanding,
        ));
        assert_eq!(
            end.failure,
            if late {
                url_support::download(unityrs_batch::http::Detail::Timeout)
            } else {
                None
            }
        );
        assert_eq!(end.facts.map(|f| f.size), if late { None } else { Some(0) });
        assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(end.integrity, None);
        if !late {
            assert_eq!(
                end.facts.unwrap().sha256.as_slice(),
                <sha2::Sha256 as sha2::Digest>::digest(b"").as_slice()
            );
        }
        assert_eq!(end.claimed_bytes, 0);
        assert_eq!(scene.counter.used(), 0);
        assert_eq!(permits.snapshot().outstanding, 0);
        assert!(
            disk.events
                .lock()
                .unwrap()
                .iter()
                .all(|e| e.operation != "write")
        );
        assert_eq!(
            disk.available_calls
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }
}
