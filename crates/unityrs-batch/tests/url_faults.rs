//! Raw parser faults cross the complete stage and retry/reset boundary.
mod http_support;
mod stage_support;
mod url_support;
use unityrs_batch::{
    http::{Detail, Error},
    stage::{DataFailure, Failure},
};
use url_support::{Reply, Scene, download};

#[test]
fn raw_body_faults_retry_and_keep_partial_facts_null() {
    for raw in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\ninvalid\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n0\r\ninvalid\r\n\r\n",
    ] {
        for attempts in [1, 2] {
            let mut scene = Scene::new(vec![Reply::raw(raw.to_vec()), Reply::ok(b"ok")]);
            scene.policy.attempts = attempts;
            let (end, _) = scene.execute();
            assert_eq!(
                end.failure,
                if attempts == 1 {
                    download(Detail::Truncated)
                } else {
                    None
                }
            );
            assert_eq!(end.facts.is_some(), attempts == 2);
            assert_eq!(scene.requests(), attempts as usize);
        }
    }
    // Capture Retry-After from the final 200 even if its body fails later.
    let mut scene = Scene::new(vec![
        Reply::raw(b"HTTP/1.1 200 OK\r\nRetry-After: 3\r\nContent-Length: 5\r\n\r\nabc".to_vec()),
        Reply::ok(b"ok"),
    ]);
    scene.policy.attempts = 2;
    assert_eq!(scene.execute().0.failure, None);
    assert_eq!(scene.peer.seen.lock().unwrap()[1].at, 3000);
}

#[test]
fn raw_head_limits_status_precedence_and_url_free_errors() {
    for (raw, detail) in [
        (
            http_support::large_head(131_073, true),
            Detail::HeadTooLarge,
        ),
        (b"not HTTP\r\n\r\n".to_vec(), Detail::NoResponse),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 18446744073709551614\r\n\r\n".to_vec(),
            Detail::HeadTooLarge,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 18446744073709551616\r\n\r\n".to_vec(),
            Detail::NoResponse,
        ),
    ] {
        let scene = Scene::new(vec![Reply::raw(raw)]);
        let (end, _) = scene.execute();
        assert_eq!(end.failure, download(detail));
        assert!(end.facts.is_none());
    }
    let scene = Scene::new(vec![Reply::raw(
        b"HTTP/1.1 503 Busy\r\nTransfer-Encoding: chunked\r\n\r\ninvalid\r\n".to_vec(),
    )]);
    assert_eq!(scene.execute().0.failure, download(Detail::Status(503)));
    for error in [
        Error::Memory,
        Error::Pool,
        Error::Busy,
        Error::InvalidRequest,
    ] {
        let mut reply = Reply::ok(b"");
        reply.failure = Some(error);
        let scene = Scene::new(vec![reply]);
        assert!(matches!(scene.execute().0.failure, Some(Failure::Run(_))));
        assert_eq!(
            scene.stop.failures.lock().unwrap()[0].1,
            unityrs_batch::control::Culprit::None
        );
    }
    for target in [
        "http://user:redirect_password@source.invalid/x?received_secret=hidden",
        "http://other.invalid/x?received_secret=hidden",
    ] {
        let mut scene = Scene::new(vec![Reply::status(302, &format!("Location: {target}\r\n"))]);
        scene.bundle["headers"] = serde_json::json!({"X-Key":"literal_header_secret"});
        let (end, _) = scene.execute();
        assert_eq!(
            end.failure,
            Some(Failure::Data(DataFailure::RedirectRefused(302)))
        );
        let rendered = format!("{end:?} {}", Detail::Status(302));
        for secret in [
            "original_secret",
            "received_secret",
            "redirect_password",
            "literal_header_secret",
            "source.invalid",
            "other.invalid",
        ] {
            assert!(!rendered.contains(secret));
        }
    }
}

#[test]
fn retry_after_date_is_evaluated_after_the_late_body_failure() {
    let mut reply = Reply::raw(b"HTTP/1.1 200 OK\r\nRetry-After: Sun, 06 Nov 1994 08:49:40 GMT\r\nContent-Length: 5\r\n\r\nabc".to_vec());
    reply.body_at = vec![5000];
    let mut scene = Scene::new(vec![reply, Reply::ok(b"ok")]);
    scene.policy.attempts = 2;
    assert_eq!(scene.execute().0.failure, None);
    assert_eq!(scene.peer.seen.lock().unwrap()[1].at, 5500);
}

#[test]
fn oversized_trailers_and_mid_chunk_resets_cross_retry_reset_accounting() {
    let prefix = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n";
    let mut too_large = prefix.to_vec();
    too_large.extend_from_slice(b"x: ");
    too_large.resize(prefix.len() + 131_068, b'a');
    too_large.extend_from_slice(b"\r\n\r\n");
    let mut too_many = prefix.to_vec();
    too_many.extend_from_slice("x: y\r\n".repeat(101).as_bytes());
    too_many.extend_from_slice(b"\r\n");
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n0\r\n\r\n";
    let mut reset = Reply::raw(raw.to_vec());
    reset.reset_at = Some(raw.windows(5).position(|s| s == b"abcde").unwrap() + 3);
    for reply in [Reply::raw(too_large), Reply::raw(too_many), reset] {
        for attempts in [1, 2] {
            let mut scene = Scene::new(vec![reply.clone(), Reply::ok(b"12345")]);
            scene.bundle["size"] = serde_json::json!(5);
            scene.policy.attempts = attempts;
            scene.policy.backoff_initial_ms = 0;
            scene.policy.backoff_max_ms = 0;
            let disk = url_support::disk::Disk::new(scene.counter.clone(), scene.time.clone());
            let (end, stage) =
                scene.with_fs(std::sync::Arc::new(url_support::disk::Fs(disk.clone())));
            assert_eq!(
                end.failure,
                if attempts == 1 {
                    download(Detail::Truncated)
                } else {
                    None
                }
            );
            assert_eq!(end.integrity, None);
            assert_eq!(scene.requests(), attempts as usize);
            assert_eq!(end.claimed_bytes, if attempts == 1 { 0 } else { 4096 });
            assert_eq!(scene.counter.used(), end.claimed_bytes);
            assert_eq!(
                scene.peer.live.load(std::sync::atomic::Ordering::Acquire),
                0
            );
            assert!(scene.time.waits.lock().unwrap().is_empty());
            let events = disk.events.lock().unwrap();
            if attempts == 1 {
                assert_eq!(end.facts, None);
                assert!(events.iter().all(|e| e.operation != "truncate"));
            } else {
                let facts = end.facts.unwrap();
                assert_eq!(facts.size, 5);
                assert_eq!(
                    facts.sha256.as_slice(),
                    <sha2::Sha256 as sha2::Digest>::digest(b"12345").as_slice()
                );
                let truncated = events.iter().find(|e| e.operation == "truncate").unwrap();
                assert_eq!((truncated.claim, truncated.length), (4096, 3));
                assert_eq!(
                    std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
                    b"12345"
                );
                assert_eq!(scene.peer.seen.lock().unwrap()[1].at, 0);
            }
        }
    }
}

#[test]
fn each_ready_data_frame_refreshes_idle_and_credits_its_own_rate_window() {
    for (second_at, expected) in [(1001, None), (1999, download(Detail::Timeout))] {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut raw = head.to_vec();
        raw.extend_from_slice(b"1\r\na\r\n1\r\nb\r\n0\r\n\r\n");
        let mut reply = Reply::raw(raw);
        reply.read_at = vec![(head.len(), 999), (head.len() + 6, second_at)];
        let mut scene = Scene::new(vec![reply]);
        scene.policy.idle_timeout_ms = 1000;
        scene.policy.min_rate_bytes = 1;
        let (end, stage) = scene.execute();
        assert_eq!(end.failure, expected);
        assert_eq!(scene.requests(), 1);
        assert_eq!(end.integrity, None);
        assert_eq!(end.claimed_bytes, if expected.is_none() { 4096 } else { 0 });
        assert_eq!(scene.counter.used(), end.claimed_bytes);
        if expected.is_none() {
            let facts = end.facts.unwrap();
            assert_eq!(facts.size, 2);
            assert_eq!(
                facts.sha256.as_slice(),
                <sha2::Sha256 as sha2::Digest>::digest(b"ab").as_slice()
            );
            assert_eq!(
                std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
                b"ab"
            );
        } else {
            assert_eq!(end.facts, None);
        }
    }
}
