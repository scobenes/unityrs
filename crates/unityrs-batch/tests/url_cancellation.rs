//! Cancellation freezes network ownership and retains every reserved claim.
mod http_support;
mod stage_support;
mod url_support;
use std::{
    future::{Future, poll_fn},
    sync::{Arc, atomic::Ordering},
    task::Poll,
};
use unityrs_batch::{
    http::permit::PermitPool,
    spool::{Host, OwnedFile},
    stage::{timers::Context, url::Stage},
};
use url_support::{Reply, Scene};

#[test]
fn cancellation_during_connect_head_and_body_keeps_the_transport_until_retirement() {
    for phase in [1, 2, 3] {
        let mut reply = Reply::ok(b"abc");
        match phase {
            1 => reply.connect_at = 100,
            2 => reply.head_at = 100,
            3 => reply.body_at = vec![100],
            _ => unreachable!(),
        }
        let mut scene = Scene::new(vec![reply]);
        scene.bundle["size"] = serde_json::json!(3);
        scene.policy.attempts = 3;
        let validated = stage_support::from_bundles(std::slice::from_ref(&scene.bundle), &[]);
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
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        {
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
            // A pending result has no received facts or failure to publish.
            runtime.block_on(poll_fn(|task| {
                assert!(future.as_mut().poll(task).is_pending());
                if scene.peer.phase.load(Ordering::Acquire) == phase {
                    Poll::Ready(())
                } else {
                    task.waker().wake_by_ref();
                    Poll::Pending
                }
            }));
            assert_eq!(scene.peer.live.load(Ordering::Acquire), 1);
            scene.stop.stopped.store(true, Ordering::Release);
            scene.time.at(100);
            runtime.block_on(poll_fn(|task| {
                assert!(future.as_mut().poll(task).is_pending());
                Poll::Ready(())
            }));
            assert_eq!(scene.requests(), 1);
            assert_eq!(scene.counter.used(), 4096);
            assert_eq!(scene.peer.live.load(Ordering::Acquire), 1);
            assert!(scene.stop.failures.lock().unwrap().is_empty());
        }
        runtime.block_on(stage.quiesce()).unwrap();
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
        assert_eq!(stage.file.as_ref().unwrap().claimed(), 4096);
        assert_eq!(stage.file.as_ref().unwrap().length(), 0);
        assert!(!stage.file.as_ref().unwrap().is_sealed());
        assert_eq!(permits.snapshot().outstanding, 0);
        stage.file.as_mut().unwrap().remove().unwrap();
        assert_eq!(scene.counter.used(), 0);
    }
}

#[test]
fn cancellation_before_sealing_retains_the_complete_unsealed_file_without_facts() {
    let scene = Scene::new(vec![Reply::ok(b"abc")]);
    let disk = url_support::disk::Disk::new(scene.counter.clone(), scene.time.clone());
    *disk.stop_on_flush.lock().unwrap() = Some(scene.stop.clone());
    let (end, stage) = scene.with_fs(Arc::new(url_support::disk::Fs(disk.clone())));
    assert_eq!(end.failure, Some(unityrs_batch::stage::Failure::Stopped));
    assert_eq!(end.facts, None);
    assert_eq!(end.integrity, None);
    assert_eq!(end.claimed_bytes, 4096);
    assert_eq!(scene.requests(), 1);
    assert_eq!(scene.counter.used(), 4096);
    assert!(scene.stop.failures.lock().unwrap().is_empty());
    assert!(
        disk.events
            .lock()
            .unwrap()
            .iter()
            .all(|e| e.operation != "seal")
    );
    let file = stage.file.as_ref().unwrap();
    assert_eq!(file.length(), 3);
    assert!(!file.is_sealed());
}

#[test]
fn cancellation_during_the_outstanding_write_retains_its_owners() {
    let scene = Scene::new(vec![Reply::ok(b"abc")]);
    let disk = url_support::disk::Disk::new(scene.counter.clone(), scene.time.clone());
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
        cutoff: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    {
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
            *disk.waiter.lock().unwrap() = Some(task.waker().clone());
            assert!(future.as_mut().poll(task).is_pending());
            if disk.entered.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));
        scene.stop.stopped.store(true, Ordering::Release);
        runtime.block_on(poll_fn(|task| {
            assert!(future.as_mut().poll(task).is_pending());
            Poll::Ready(())
        }));
        assert_eq!(scene.requests(), 1);
        assert_eq!(scene.counter.used(), 4096);
        assert_eq!(permits.snapshot().outstanding, 1);
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 1);
    }
    assert!(stage.file.is_none());
    assert!(permit.charged() > unityrs_batch::stage::url::STAGE_ENVELOPE);
    disk.release();
    runtime.block_on(stage.quiesce()).unwrap();
    assert_eq!(permits.snapshot().outstanding, 0);
    assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
    let file = stage.file.as_mut().unwrap();
    assert_eq!(file.length(), 3);
    assert_eq!(file.claimed(), 4096);
    assert!(!file.is_sealed());
    file.remove().unwrap();
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn stop_during_retry_wait_keeps_the_reset_owner_and_never_starts_another_try() {
    let mut scene = Scene::new(vec![
        Reply::raw("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab"),
        Reply::ok(b"abc"),
    ]);
    scene.bundle["size"] = serde_json::json!(3);
    scene.policy.attempts = 3;
    let disk = url_support::disk::Disk::new(scene.counter.clone(), scene.time.clone());
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
        cutoff: None,
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
            *disk.waiter.lock().unwrap() = Some(task.waker().clone());
            let result = future.as_mut().poll(task);
            if disk
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.operation == "truncate")
            {
                assert!(result.is_pending());
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        assert_eq!(scene.requests(), 1);
        assert_eq!(scene.counter.used(), 0);
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
        scene.stop.stopped.store(true, Ordering::Release);
        scene.time.at(500);
        future.await
    });
    assert_eq!(end.failure, Some(unityrs_batch::stage::Failure::Stopped));
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    assert_eq!(scene.requests(), 1);
    assert_eq!(permits.snapshot().outstanding, 0);
    assert!(scene.stop.failures.lock().unwrap().is_empty());
    let events = disk.events.lock().unwrap();
    let resets: Vec<_> = events
        .iter()
        .filter(|e| e.operation == "truncate")
        .collect();
    assert_eq!(resets.len(), 1);
    assert_eq!((resets[0].claim, resets[0].length), (4096, 2));
    assert!(events.iter().all(|e| e.operation != "seal"));
    let file = stage.file.as_ref().unwrap();
    assert_eq!(
        (file.length(), file.claimed(), file.is_sealed()),
        (0, 0, false)
    );
}
