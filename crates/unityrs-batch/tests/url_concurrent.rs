//! Two stages share one runtime and clock, with independent stop controls.
#[allow(dead_code)]
#[path = "http_support/count.rs"]
mod count;
mod http_support;
mod stage_support;
mod url_support;
use std::{
    future::{Future, poll_fn},
    sync::{Arc, atomic::Ordering},
    task::Poll,
    time::Duration,
};
use unityrs_batch::{
    alloc::Counting,
    http::{
        Error, Payload,
        permit::{OperationToken, PermitPool, RequestPermit},
    },
    spool::{Host, OwnedFile},
    stage::{
        StageEnd,
        timers::{Clock, Context},
        url::{Stage, Transport},
    },
};
use url_support::{Peer, Reply, ReplyBody, Scene};
#[global_allocator]
static ALLOCATOR: Counting<count::Network> = Counting(count::Network);

fn pair<A: Transport<Response = ReplyBody>, B: Transport<Response = ReplyBody>>(
    first: &Scene,
    second: &Scene,
    a: &A,
    b: &B,
) -> (StageEnd, StageEnd) {
    pair_files(first, second, a, b, Arc::new(Host), Arc::new(Host))
}
fn pair_files<A: Transport<Response = ReplyBody>, B: Transport<Response = ReplyBody>>(
    first: &Scene,
    second: &Scene,
    a: &A,
    b: &B,
    fs1: Arc<dyn unityrs_batch::spool::FileSystem>,
    fs2: Arc<dyn unityrs_batch::spool::FileSystem>,
) -> (StageEnd, StageEnd) {
    pair_files_observed(first, second, a, b, fs1, fs2, |_| {})
}
fn pair_files_observed<A: Transport<Response = ReplyBody>, B: Transport<Response = ReplyBody>>(
    first: &Scene,
    second: &Scene,
    a: &A,
    b: &B,
    fs1: Arc<dyn unityrs_batch::spool::FileSystem>,
    fs2: Arc<dyn unityrs_batch::spool::FileSystem>,
    mut after_poll: impl FnMut(&std::task::Waker),
) -> (StageEnd, StageEnd) {
    let v1 = stage_support::from_bundles(std::slice::from_ref(&first.bundle), &[]);
    let v2 = stage_support::from_bundles(std::slice::from_ref(&second.bundle), &[]);
    let permits = PermitPool::new(2).unwrap();
    let mut p1 = permits.acquire().unwrap();
    let mut p2 = permits.acquire().unwrap();
    let file1 = OwnedFile::create(first.scratch.join("i0"), first.counter.clone(), fs1).unwrap();
    let file2 = OwnedFile::create(second.scratch.join("i0"), second.counter.clone(), fs2).unwrap();
    let mut s1 = Stage::new(file1, &p1).unwrap();
    let mut s2 = Stage::new(file2, &p2).unwrap();
    let cx1 = Context {
        clock: first.time.clone(),
        control: first.stop.clone(),
        cutoff: None,
    };
    let cx2 = Context {
        clock: first.time.clone(),
        control: second.stop.clone(),
        cutoff: None,
    };
    let drive = first.time.drive(
        async {
            let pool = tokio::runtime::Handle::current();
            let mut f1 = std::pin::pin!(s1.run(
                0,
                v1.spec.bundle(0).unwrap(),
                first.m,
                &first.policy,
                &(),
                a,
                &mut p1,
                &pool,
                &cx1
            ));
            let mut f2 = std::pin::pin!(s2.run(
                1,
                v2.spec.bundle(0).unwrap(),
                second.m,
                &second.policy,
                &(),
                b,
                &mut p2,
                &pool,
                &cx2
            ));
            let (mut end1, mut end2) = (None, None);
            poll_fn(|task| {
                if end1.is_none()
                    && let Poll::Ready(end) = f1.as_mut().poll(task)
                {
                    end1 = Some(end);
                }
                if end2.is_none()
                    && let Poll::Ready(end) = f2.as_mut().poll(task)
                {
                    end2 = Some(end);
                }
                if end1.is_some() && end2.is_some() {
                    Poll::Ready((end1.take().unwrap(), end2.take().unwrap()))
                } else {
                    Poll::Pending
                }
            })
            .await
        },
        || permits.snapshot().outstanding,
    );
    let mut drive = std::pin::pin!(drive);
    http_support::run(poll_fn(|cx| {
        let result = drive.as_mut().poll(cx);
        after_poll(cx.waker());
        result
    }))
}

/// Admit both tries at time zero before either transport can deliver events.
struct Yielding<'a>(&'a Peer);
impl Transport for Yielding<'_> {
    type Response = ReplyBody;
    async fn request(
        &self,
        permit: &mut RequestPermit,
        url: &url::Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: OperationToken,
    ) -> Result<ReplyBody, Error> {
        tokio::task::yield_now().await;
        self.0.request(permit, url, request, timeout, token).await
    }
}

#[test]
fn a_later_head_cannot_skip_the_other_stages_earlier_body() {
    for reverse in [false, true] {
        let mut late = Reply::ok(b"late");
        late.head_at = 900;
        let mut early = Reply::ok(b"early");
        early.body_at = vec![500];
        let mut first = Scene::new(vec![late]);
        let mut second = Scene::new(vec![early]);
        second.time = first.time.clone();
        second.peer.time = first.time.clone();
        first.policy.idle_timeout_ms = 1000;
        second.policy.idle_timeout_ms = 700;
        let (a, b) = if reverse {
            let (b, a) = pair(
                &second,
                &first,
                &Yielding(&second.peer),
                &Yielding(&first.peer),
            );
            (a, b)
        } else {
            pair(
                &first,
                &second,
                &Yielding(&first.peer),
                &Yielding(&second.peer),
            )
        };
        assert_eq!(a.failure, None);
        assert_eq!(b.failure, None);
        assert_eq!(a.facts.unwrap().size, 4);
        assert_eq!(b.facts.unwrap().size, 5);
        assert_eq!(first.requests(), 1);
        assert_eq!(second.requests(), 1);
        assert_eq!(first.counter.used(), 4096);
        assert_eq!(second.counter.used(), 4096);
        assert_eq!(first.time.waits.lock().unwrap().first(), Some(&500));
    }
}

#[test]
fn successive_receipts_cannot_skip_a_healthy_body_in_either_poll_order() {
    for reverse in [false, true] {
        let mut receipts = Reply::ok(b"ab");
        let head_len = receipts.raw.len() - 2;
        receipts.head_at = 100;
        receipts.read_at = vec![(head_len, 500), (head_len + 1, 700)];
        let mut healthy = Reply::ok(b"healthy");
        healthy.body_at = vec![500];
        let mut first = Scene::new(vec![receipts]);
        let mut second = Scene::new(vec![healthy]);
        second.time = first.time.clone();
        second.peer.time = first.time.clone();
        first.policy.idle_timeout_ms = 1000;
        second.policy.idle_timeout_ms = 700;
        let (a, b) = if reverse {
            let (b, a) = pair(
                &second,
                &first,
                &Yielding(&second.peer),
                &Yielding(&first.peer),
            );
            (a, b)
        } else {
            pair(
                &first,
                &second,
                &Yielding(&first.peer),
                &Yielding(&second.peer),
            )
        };
        for (scene, end, bytes) in [
            (&first, a, b"ab".as_slice()),
            (&second, b, b"healthy".as_slice()),
        ] {
            assert_eq!(end.failure, None, "reverse={reverse}");
            assert_eq!(end.integrity, None);
            let facts = end.facts.unwrap();
            assert_eq!(facts.size, bytes.len() as u64);
            assert_eq!(
                facts.sha256.as_slice(),
                <sha2::Sha256 as sha2::Digest>::digest(bytes).as_slice()
            );
            assert_eq!(std::fs::read(scene.scratch.join("i0")).unwrap(), bytes);
            assert_eq!(scene.requests(), 1);
            assert_eq!(end.claimed_bytes, 4096);
            assert_eq!(scene.counter.used(), 4096);
            assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
        }
        let events = first.time.waits.lock().unwrap();
        assert!(events.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(events.first(), Some(&100));
        assert!(events.contains(&500));
        assert!(events.contains(&700));
    }
}

struct Refusing<'a>(&'a Peer);
impl Transport for Refusing<'_> {
    type Response = ReplyBody;
    async fn request(
        &self,
        permit: &mut RequestPermit,
        url: &url::Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: OperationToken,
    ) -> Result<ReplyBody, Error> {
        // Let the other stage enter a network wait before this refusal.
        drop(token);
        self.0.time.sleep_until(Duration::from_millis(5)).await;
        count::refuse_next(12345);
        assert!(Vec::<u8>::new().try_reserve_exact(12345).is_err());
        let token = permit
            .begin(unityrs_batch::http::permit::Operation::Lookup)
            .unwrap();
        self.0.request(permit, url, request, timeout, token).await
    }
}

#[test]
fn another_stages_refusal_does_not_taint_a_success() {
    let mut reply = Reply::ok(b"clean");
    reply.head_at = 10;
    let first = Scene::new(vec![reply]);
    let mut second = Scene::new(vec![Reply::ok(b"refused")]);
    second.time = first.time.clone();
    second.peer.time = first.time.clone();
    let (clean, refused) = pair(&first, &second, &first.peer, &Refusing(&second.peer));
    assert_eq!(clean.failure, None);
    assert_eq!(clean.facts.unwrap().size, 5);
    assert_eq!(clean.claimed_bytes, 4096);
    assert_eq!(first.peer.live.load(Ordering::Acquire), 0);
    assert!(first.stop.failures.lock().unwrap().is_empty());
    assert_eq!(
        refused.failure,
        Some(unityrs_batch::stage::Failure::Run(
            unityrs_batch::job::RunFailure::new(unityrs_batch::attempt::ReasonCode::HostMemory)
        ))
    );
    assert!(refused.facts.is_none());
    assert_eq!(refused.claimed_bytes, 4096);
}

#[test]
fn scheduled_write_completion_cannot_skip_a_healthy_concurrent_body_in_either_poll_order() {
    for reverse in [false, true] {
        let mut first = Scene::new(vec![Reply::ok(b"held")]);
        let mut reply = Reply::ok(b"healthy");
        reply.body_at = vec![500];
        let mut second = Scene::new(vec![reply]);
        second.time = first.time.clone();
        second.peer.time = first.time.clone();
        first.policy.idle_timeout_ms = 700;
        second.policy.idle_timeout_ms = 700;
        let disk = url_support::disk::Disk::new(first.counter.clone(), first.time.clone());
        disk.mode.store(4, Ordering::Release);
        let fs = Arc::new(url_support::disk::Fs(disk));
        let (a, b) = if reverse {
            let (b, a) = pair_files(
                &second,
                &first,
                &Yielding(&second.peer),
                &Yielding(&first.peer),
                Arc::new(Host),
                fs,
            );
            (a, b)
        } else {
            pair_files(
                &first,
                &second,
                &Yielding(&first.peer),
                &Yielding(&second.peer),
                fs,
                Arc::new(Host),
            )
        };
        for (scene, end, bytes) in [
            (&first, a, b"held".as_slice()),
            (&second, b, b"healthy".as_slice()),
        ] {
            assert_eq!(end.failure, None, "reverse={reverse}");
            assert_eq!(end.integrity, None);
            let facts = end.facts.unwrap();
            assert_eq!(facts.size, bytes.len() as u64);
            assert_eq!(
                facts.sha256.as_slice(),
                <sha2::Sha256 as sha2::Digest>::digest(bytes).as_slice()
            );
            assert_eq!(scene.requests(), 1);
            assert_eq!(end.claimed_bytes, 4096);
            assert_eq!(scene.counter.used(), 4096);
            assert_eq!(std::fs::read(scene.scratch.join("i0")).unwrap(), bytes);
            assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
        }
        let events = first.time.waits.lock().unwrap();
        assert!(events.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(events.contains(&500));
        assert!(events.contains(&5000));
    }
}

#[test]
fn an_unpublished_write_result_cannot_advance_past_its_join_receipt() {
    for reverse in [false, true] {
        let mut first = Scene::new(vec![Reply::ok(b"held")]);
        let mut reply = Reply::ok(b"healthy");
        reply.body_at = vec![500];
        let mut second = Scene::new(vec![reply]);
        second.time = first.time.clone();
        second.peer.time = first.time.clone();
        first.policy.idle_timeout_ms = 700;
        second.policy.idle_timeout_ms = 700;
        let disk = url_support::disk::Disk::new(first.counter.clone(), first.time.clone());
        disk.mode.store(4, Ordering::Release);
        let fs = Arc::new(url_support::disk::Fs(disk));
        let barrier = first.time.hold_return_at(500);
        let mut held_on_previous_poll = false;
        let mut observed = false;
        let after_poll = |wake: &std::task::Waker| {
            if barrier.entered.load(Ordering::Acquire) && !observed {
                if !held_on_previous_poll {
                    held_on_previous_poll = true;
                    // Require a complete driver poll with B already held;
                    // entry may have raced this poll's outstanding snapshot.
                    wake.wake_by_ref();
                    return;
                }
                observed = true;
                let at = first.time.ms.load(Ordering::Acquire);
                // Release before asserting, including on the failing path.
                barrier.release();
                assert_eq!(at, 500, "B's unreceived write must hold A at 500 ms");
            }
        };
        let (a, b) = if reverse {
            let (b, a) = pair_files_observed(
                &second,
                &first,
                &Yielding(&second.peer),
                &Yielding(&first.peer),
                Arc::new(Host),
                fs,
                after_poll,
            );
            (a, b)
        } else {
            pair_files_observed(
                &first,
                &second,
                &Yielding(&first.peer),
                &Yielding(&second.peer),
                fs,
                Arc::new(Host),
                after_poll,
            )
        };
        assert!(observed, "B must reach the worker-return barrier");
        assert_eq!(a.failure, None, "reverse={reverse}");
        assert_eq!(b.failure, None, "B must reach EOF without a timeout");
        assert_eq!(b.facts.unwrap().size, 7);
        assert_eq!(
            std::fs::read(second.scratch.join("i0")).unwrap(),
            b"healthy"
        );
        assert!(first.time.waits.lock().unwrap().contains(&5000));
    }
}
