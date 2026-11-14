mod http_support;
mod stage_support;
mod url_support;
use serde_json::json;
use std::{
    future::{Future, poll_fn},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use unityrs_batch::{
    attempt::ReasonCode,
    control::Culprit,
    http::{Detail, permit::PermitPool},
    spool::OwnedFile,
    stage::{Failure, timers::Context, url::Stage},
};
use url_support::{Reply, Scene, disk::Disk, download};

#[test]
fn retry_truncates_one_path_releases_then_reserves_and_restarts_at_zero() {
    let mut scene = Scene::new(vec![
        Reply::raw(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc".to_vec()),
        Reply::ok(b"12345"),
    ]);
    scene.bundle["size"] = json!(5);
    scene.policy.attempts = 2;
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    disk.mode.store(1, Ordering::Release); // EINTR then partial positional writes
    let (end, stage) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
    assert_eq!(end.failure, None);
    let events = disk.events.lock().unwrap();
    let truncate = events
        .iter()
        .position(|e| e.operation == "truncate")
        .unwrap();
    assert_eq!(events[truncate].claim, 4096);
    assert_eq!(events[truncate].length, 3);
    assert_eq!(events[truncate + 1].operation, "write");
    assert_eq!(events[truncate + 1].length, 0);
    assert_eq!(events[truncate + 1].claim, 4096);
    assert_eq!(
        std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
        b"12345"
    );
    assert_eq!(scene.counter.used(), 4096);
    assert_eq!(scene.requests(), 2);
}

#[test]
fn incremental_claims_round_cumulative_extents_and_never_claim_content_length() {
    let mut reply = Reply::ok(&vec![b'x'; 100_000]);
    reply.fragment = 3000;
    let scene = Scene::new(vec![reply]);
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    let (end, _) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
    assert_eq!(end.failure, None);
    assert_eq!(scene.peer.seen.lock().unwrap()[0].claimed, 0);
    assert_eq!(end.claimed_bytes, 102_400);
    let events = disk.events.lock().unwrap();
    let writes = events
        .iter()
        .filter(|e| e.operation == "write")
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].claim, 65_536);
    assert!(
        writes
            .iter()
            .all(|e| e.claim <= 102_400 && e.claim >= e.length)
    );
}

#[test]
fn failed_write_truncate_seal_claim_delete_and_pool_errors_stop_without_a_culprit() {
    for (mode, reason) in [
        (2, ReasonCode::SpoolFull),
        (3, ReasonCode::HostIo),
        (5, ReasonCode::HostIo),
        (6, ReasonCode::HostIo),
        (7, ReasonCode::HostIo),
        (8, ReasonCode::Internal),
        (9, ReasonCode::HostIo),
    ] {
        let reply = if mode == 3 {
            Reply::raw(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc".to_vec())
        } else if mode == 6 {
            Reply::status(404, "")
        } else {
            Reply::ok(b"12345")
        };
        let mut scene = Scene::new(vec![reply]);
        scene.bundle["size"] = json!(5);
        scene.policy.attempts = 2;
        let disk = Disk::new(scene.counter.clone(), scene.time.clone());
        disk.mode.store(mode, Ordering::Release);
        let (end, stage) = scene.with_fs(Arc::new(url_support::disk::Fs(disk)));
        assert!(
            matches!(end.failure, Some(Failure::Run(_) | Failure::Stopped)),
            "mode {mode}: {end:?}"
        );
        assert_eq!(
            scene.stop.failures.lock().unwrap().last().unwrap().0.reason,
            reason
        );
        assert!(end.facts.is_none());
        assert!(
            scene
                .stop
                .failures
                .lock()
                .unwrap()
                .iter()
                .all(|(_, culprit)| *culprit == Culprit::None)
        );
        assert!(!scene.stop.failures.lock().unwrap().is_empty());
        assert_eq!(scene.counter.used(), if mode == 7 { 0 } else { 4096 });
        if mode == 3 {
            assert_eq!(
                std::fs::metadata(stage.file.as_ref().unwrap().path())
                    .unwrap()
                    .len(),
                0
            );
        }
        assert_eq!(scene.requests(), usize::from(mode != 7));
    }
}

#[test]
fn a_write_longer_than_idle_pauses_only_its_own_active_time() {
    let mut scene = Scene::new(vec![Reply::ok(b"abc")]);
    scene.policy.idle_timeout_ms = 1000;
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    disk.mode.store(4, Ordering::Release);
    assert_eq!(
        scene
            .with_fs(Arc::new(url_support::disk::Fs(disk)))
            .0
            .failure,
        None
    );
    assert_eq!(scene.time.ms.load(Ordering::Acquire), 5000);
    // A second stage on the same clock receives no pause credit.
    let mut late = Reply::ok(b"x");
    late.head_at = 5000;
    let mut other = Scene::new(vec![late]);
    other.policy.idle_timeout_ms = 1000;
    assert_eq!(other.execute().0.failure, download(Detail::Timeout));
}

#[test]
fn stop_during_held_write_retains_task_file_buffer_claim_and_permit_until_quiescence() {
    let scene = Scene::new(vec![Reply::ok(b"abc")]);
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    disk.held.store(true, Ordering::Release);
    let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
    let permits = PermitPool::new(1).unwrap();
    let mut permit = permits.acquire().unwrap();
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
        cutoff: Some(Duration::from_millis(10)),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let pool = tokio::runtime::Handle::current();
        let mut future = std::pin::pin!(stage.run(
            0,
            validated.spec.bundle(0).unwrap(),
            scene.m,
            &scene.policy,
            &(),
            &scene.peer,
            &mut permit,
            &pool,
            &cx
        ));
        let end = poll_fn(|task| {
            *disk.waiter.lock().unwrap() = Some(task.waker().clone());
            if disk.entered.load(Ordering::Acquire) {
                scene.time.at(10);
            }
            future.as_mut().poll(task)
        })
        .await;
        assert_eq!(end.failure, Some(Failure::Stopped));
        assert_eq!(end.facts, None);
        assert_eq!(end.integrity, None);
        assert_eq!(end.claimed_bytes, 4096);
    });
    assert!(stage.file.is_none());
    assert_eq!(permits.snapshot().outstanding, 1);
    assert!(permit.charged() > unityrs_batch::stage::url::STAGE_ENVELOPE);
    assert_eq!(scene.counter.used(), 4096);
    disk.release();
    runtime.block_on(stage.quiesce()).unwrap();
    assert_eq!(permits.snapshot().outstanding, 0);
    assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
    assert_eq!(
        std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
        b"abc"
    );
    assert_eq!(scene.requests(), 1);
    stage.file.as_mut().unwrap().remove().unwrap();
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn lookup_token_wait_rechecks_cutoff_without_starting_a_try() {
    let scene = Scene::new(vec![]);
    let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
    let permits = PermitPool::new(1).unwrap();
    let mut permit = permits.acquire().unwrap();
    let mut token = Some(
        permit
            .begin(unityrs_batch::http::permit::Operation::Lookup)
            .unwrap(),
    );
    let file = OwnedFile::create(
        scene.scratch.join("i0"),
        scene.counter.clone(),
        Arc::new(unityrs_batch::spool::Host),
    )
    .unwrap();
    let mut stage = Stage::<url_support::ReplyBody>::new(file, &permit).unwrap();
    let cx = Context {
        clock: scene.time.clone(),
        control: scene.stop.clone(),
        cutoff: Some(Duration::from_millis(10)),
    };
    let end = http_support::run(async {
        let pool = tokio::runtime::Handle::current();
        let mut future = std::pin::pin!(stage.run(
            0,
            validated.spec.bundle(0).unwrap(),
            scene.m,
            &scene.policy,
            &(),
            &scene.peer,
            &mut permit,
            &pool,
            &cx
        ));
        poll_fn(|task| {
            let result = future.as_mut().poll(task);
            if result.is_pending() && token.is_some() {
                scene.time.at(10);
                drop(token.take());
                task.waker().wake_by_ref();
            }
            result
        })
        .await
    });
    assert_eq!(end.failure, Some(Failure::Stopped));
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    let file = stage.file.as_ref().unwrap();
    assert_eq!(
        (file.length(), file.claimed(), file.is_sealed()),
        (0, 0, false)
    );
    assert_eq!(scene.requests(), 0);
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn cutoff_is_checked_after_actual_lookup_token_acquisition() {
    struct AcquisitionClock(Arc<PermitPool>);
    impl unityrs_batch::stage::timers::Clock for AcquisitionClock {
        fn now(&self) -> Duration {
            Duration::from_millis(if self.0.snapshot().outstanding == 0 {
                0
            } else {
                10
            })
        }
        fn wall(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH
        }
        async fn sleep_until(&self, _: Duration) {
            std::future::pending::<()>().await;
        }
    }
    let scene = Scene::new(vec![]);
    let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
    let permits = Arc::new(PermitPool::new(1).unwrap());
    let mut permit = permits.acquire().unwrap();
    let file = OwnedFile::create(
        scene.scratch.join("i0"),
        scene.counter.clone(),
        Arc::new(unityrs_batch::spool::Host),
    )
    .unwrap();
    let mut stage = Stage::<url_support::ReplyBody>::new(file, &permit).unwrap();
    let cx = Context {
        clock: Arc::new(AcquisitionClock(permits.clone())),
        control: scene.stop.clone(),
        cutoff: Some(Duration::from_millis(10)),
    };
    let end = http_support::run(async {
        stage
            .run(
                0,
                validated.spec.bundle(0).unwrap(),
                scene.m,
                &scene.policy,
                &(),
                &scene.peer,
                &mut permit,
                &tokio::runtime::Handle::current(),
                &cx,
            )
            .await
    });
    assert_eq!(end.failure, Some(Failure::Stopped));
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    let file = stage.file.as_ref().unwrap();
    assert_eq!(
        (file.length(), file.claimed(), file.is_sealed()),
        (0, 0, false)
    );
    assert_eq!(scene.requests(), 0);
    assert_eq!(permits.snapshot().outstanding, 0);
}

#[test]
fn one_stages_write_pause_does_not_pause_a_concurrent_stalled_stage() {
    let own = Scene::new(vec![Reply::ok(b"abc")]);
    let mut stalled = Reply::ok(b"x");
    stalled.stall = true;
    let mut other = Scene::new(vec![stalled]);
    other.time = own.time.clone();
    other.peer.time = own.time.clone();
    let disk = Disk::new(own.counter.clone(), own.time.clone());
    disk.mode.store(4, Ordering::Release);
    let first = stage_support::from_bundles(std::slice::from_ref(&own.bundle), &[]);
    let second = stage_support::from_bundles(std::slice::from_ref(&other.bundle), &[]);
    let permits = PermitPool::new(2).unwrap();
    let mut p1 = permits.acquire().unwrap();
    let mut p2 = permits.acquire().unwrap();
    let f1 = OwnedFile::create(
        own.scratch.join("i0"),
        own.counter.clone(),
        Arc::new(url_support::disk::Fs(disk)),
    )
    .unwrap();
    let f2 = OwnedFile::create(
        other.scratch.join("i0"),
        other.counter.clone(),
        Arc::new(unityrs_batch::spool::Host),
    )
    .unwrap();
    let mut s1 = Stage::new(f1, &p1).unwrap();
    let mut s2 = Stage::new(f2, &p2).unwrap();
    let cx1 = Context {
        clock: own.time.clone(),
        control: own.stop.clone(),
        cutoff: None,
    };
    let cx2 = Context {
        clock: own.time.clone(),
        control: other.stop.clone(),
        cutoff: None,
    };
    let policy = unityrs_batch::spec::model::Http {
        idle_timeout_ms: 1000,
        attempts: 1,
        ..Default::default()
    };
    let (mut a, mut b) = (None, None);
    http_support::run(own.time.drive(
        async {
            let pool = tokio::runtime::Handle::current();
            let mut f1 = std::pin::pin!(s1.run(
                0,
                first.spec.bundle(0).unwrap(),
                own.m,
                &policy,
                &(),
                &own.peer,
                &mut p1,
                &pool,
                &cx1
            ));
            let mut f2 = std::pin::pin!(s2.run(
                0,
                second.spec.bundle(0).unwrap(),
                other.m,
                &policy,
                &(),
                &other.peer,
                &mut p2,
                &pool,
                &cx2
            ));
            poll_fn(|task| {
                if b.is_none()
                    && let std::task::Poll::Ready(end) = f2.as_mut().poll(task)
                {
                    b = Some(end);
                }
                if a.is_none()
                    && let std::task::Poll::Ready(end) = f1.as_mut().poll(task)
                {
                    a = Some(end);
                }
                if a.is_some() && b.is_some() {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            })
            .await;
        },
        || permits.snapshot().outstanding,
    ));
    assert_eq!(a.unwrap().failure, None);
    assert_eq!(b.unwrap().failure, download(Detail::Timeout));
}

#[test]
fn an_oversize_piece_is_rejected_before_its_claim_or_write() {
    let mut reply = Reply::raw(b"HTTP/1.1 200 OK\r\n\r\nabc".to_vec());
    reply.fragment = 1;
    let mut scene = Scene::new(vec![reply]);
    scene.bundle["size"] = json!(2);
    scene.m = 2;
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    let (end, _) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
    assert_eq!(
        end.failure,
        Some(Failure::Data(
            unityrs_batch::stage::DataFailure::InputBytes(2)
        ))
    );
    assert!(end.facts.is_none());
    assert_eq!(
        disk.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.operation == "write")
            .count(),
        0
    );
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn a_network_cutoff_parks_the_in_flight_future_until_the_outer_owner_retires_it() {
    let mut reply = Reply::ok(b"abc");
    reply.head_at = 10;
    let scene = Scene::new(vec![reply]);
    let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
    let permits = PermitPool::new(1).unwrap();
    let mut permit = permits.acquire().unwrap();
    let file = OwnedFile::create(
        scene.scratch.join("i0"),
        scene.counter.clone(),
        Arc::new(unityrs_batch::spool::Host),
    )
    .unwrap();
    let mut stage = Stage::new(file, &permit).unwrap();
    let cx = Context {
        clock: scene.time.clone(),
        control: scene.stop.clone(),
        cutoff: Some(Duration::from_millis(10)),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    {
        // Future ownership is outside the drive scope, as required at handover.
        let mut future = std::pin::pin!(stage.run(
            0,
            validated.spec.bundle(0).unwrap(),
            scene.m,
            &scene.policy,
            &(),
            &scene.peer,
            &mut permit,
            runtime.handle(),
            &cx
        ));
        runtime.block_on(poll_fn(|task| {
            assert!(future.as_mut().poll(task).is_pending());
            scene.time.at(10);
            assert!(future.as_mut().poll(task).is_pending());
            std::task::Poll::Ready(())
        }));
        assert_eq!(scene.requests(), 1);
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 1);
        assert_eq!(permits.snapshot().occupied, 1);
        // Even repeated accidental polls cannot start another hop or retry.
        runtime.block_on(poll_fn(|task| {
            assert!(future.as_mut().poll(task).is_pending());
            std::task::Poll::Ready(())
        }));
        assert_eq!(scene.requests(), 1);
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 1);
        // Cooperative retirement after the assertion; production retains this
        // future until B9's exit path, rather than dropping it at handover.
    }
    assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
    runtime.block_on(stage.quiesce()).unwrap();
    assert_eq!(permits.snapshot().outstanding, 0);
}

#[test]
fn claim_time_counts_from_lookup_acquisition_and_is_not_a_write_pause() {
    let mut scene = Scene::new(vec![]);
    scene.policy.idle_timeout_ms = 1000;
    scene.bundle["size"] = json!(1);
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    disk.mode.store(10, Ordering::Release);
    assert_eq!(
        scene
            .with_fs(Arc::new(url_support::disk::Fs(disk)))
            .0
            .failure,
        download(Detail::Timeout)
    );
    assert_eq!(scene.requests(), 0);
}

#[test]
fn immediately_ready_one_byte_chunks_share_bounded_positional_writes() {
    let bytes = 2 * unityrs_batch::http::request::PIECE + 17;
    let raw = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{}0\r\n\r\n",
        "1\r\nx\r\n".repeat(bytes)
    );
    let scene = Scene::new(vec![Reply::raw(raw)]);
    let disk = Disk::new(scene.counter.clone(), scene.time.clone());
    let (end, _) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
    assert_eq!(end.failure, None);
    assert_eq!(end.facts.unwrap().size, bytes as u64);
    let writes = disk
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.operation == "write")
        .count();
    // This peer's DATA is immediately Ready, so there are no Pending frames.
    assert!(
        writes <= bytes.div_ceil(unityrs_batch::http::request::PIECE),
        "{writes}"
    );
    assert_eq!(disk.available_calls.load(Ordering::Acquire), writes);
}

#[test]
fn every_staging_piece_checks_free_space_once_even_with_an_upfront_reservation() {
    for supplied in [false, true] {
        let mut scene = Scene::new(vec![Reply::ok(b"abc")]);
        if supplied {
            scene.bundle["size"] = json!(3);
        }
        let disk = Disk::new(scene.counter.clone(), scene.time.clone());
        let (end, _) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
        assert_eq!(end.failure, None);
        assert_eq!(end.facts.unwrap().size, 3);
        assert_eq!(end.claimed_bytes, 4096);
        assert_eq!(scene.requests(), 1);
        assert_eq!(
            disk.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.operation == "write")
                .count(),
            1
        );
        // The optional upfront reservation plus exactly one check for the
        // piece; dispatching its already-checked pool write adds none.
        assert_eq!(
            disk.available_calls.load(Ordering::Acquire),
            1 + usize::from(supplied)
        );
    }
}
