mod http_support;
mod stage_support;
mod url_support;
use serde_json::json;
use sha2::Digest;
use std::{os::unix::fs::PermissionsExt, time::Duration};
use unityrs_batch::{
    http::Detail,
    spec::model::{Redirects, RetryOn, TryClass},
    stage::{DataFailure, Failure, Integrity},
};
use url_support::{Reply, Scene, download};

#[test]
fn successful_framings_digest_mode_claim_and_request_defaults() {
    for reply in [Reply::ok(b"abc"), Reply::raw(b"HTTP/1.1 200 OK\r\n\r\nabc".to_vec()),
        Reply::raw(b"HTTP/1.1 103 Early Hints\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n2\r\nbc\r\n0\r\n\r\n".to_vec())] {
        let mut scene = Scene::new(vec![reply]);
        scene.bundle["size"] = json!(3);
        scene.bundle["sha256"] = json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let (end, stage) = scene.execute();
        assert_eq!(end.failure, None);
        let facts = end.facts.unwrap();
        assert_eq!(facts.size, 3);
        assert_eq!(facts.sha256.as_slice(), sha2::Sha256::digest(b"abc").as_slice());
        let file = stage.file.as_ref().unwrap();
        assert_eq!(std::fs::read(file.path()).unwrap(), b"abc");
        assert_eq!(std::fs::metadata(file.path()).unwrap().permissions().mode() & 0o777, 0o400);
        assert_eq!(end.claimed_bytes, 4096);
        let seen = scene.peer.seen.lock().unwrap();
        assert_eq!(seen[0].uri, "/start?original_secret=hidden");
        assert_eq!(seen[0].headers["user-agent"], concat!("unityrs/", env!("CARGO_PKG_VERSION")));
        assert_eq!(seen[0].headers["accept"], "*/*");
        assert_eq!(seen[0].headers["accept-encoding"], "identity");
        assert_eq!(seen[0].headers["host"], "source.invalid");
        assert!(!seen[0].headers.contains_key("range"));
    }
    for bytes in [b"".as_slice(), b"x"] {
        let scene = Scene::new(vec![Reply::ok(bytes)]);
        assert_eq!(scene.execute().0.facts.unwrap().size, bytes.len() as u64);
    }
}

#[test]
fn all_redirect_statuses_headers_origin_normalization_and_retry_restart() {
    for status in [301, 302, 303, 307, 308] {
        let mut scene = Scene::new(vec![
            Reply::status(
                status,
                "Location: http://source.invalid:80/next?new_secret=hidden\r\n",
            ),
            Reply::ok(b"ok"),
        ]);
        scene.bundle["headers"] =
            json!({"User-Agent":"custom", "Accept":"application/x-test", "X-Key":"literal-secret"});
        let (end, _) = scene.execute();
        assert_eq!(end.failure, None);
        let seen = scene.peer.seen.lock().unwrap();
        assert_eq!(seen[1].uri, "/next?new_secret=hidden");
        for request in seen.iter() {
            assert_eq!(request.headers["user-agent"], "custom");
            assert_eq!(request.headers["accept"], "application/x-test");
            assert_eq!(request.headers["x-key"], "literal-secret");
        }
    }
    let mut scene = Scene::new(vec![
        Reply::status(302, "Location: /redirected\r\nRetry-After: 20\r\n"),
        Reply::status(503, ""),
        Reply::ok(b"yes"),
    ]);
    scene.policy.attempts = 2;
    assert_eq!(scene.execute().0.failure, None);
    let seen = scene.peer.seen.lock().unwrap();
    assert_eq!(seen[2].uri, seen[0].uri);
    assert_eq!(seen[2].at, 500);
}

#[test]
fn redirect_refusals_have_only_status_and_never_retry() {
    for location in [
        "https://source.invalid/x",
        "http://other.invalid/x",
        "http://source.invalid:81/x",
        "http://user:secret@source.invalid/x",
        "ftp://source.invalid/x",
    ] {
        let mut scene = Scene::new(vec![Reply::status(
            307,
            &format!("Location: {location}\r\n"),
        )]);
        scene.policy.attempts = 10;
        let (end, _) = scene.execute();
        assert_eq!(
            end.failure,
            Some(Failure::Data(DataFailure::RedirectRefused(307)))
        );
        assert_eq!(scene.requests(), 1);
        assert!(!format!("{end:?}").contains("secret"));
    }
    let mut scene = Scene::new(vec![Reply::status(301, "Location: /next\r\n")]);
    scene.policy.redirects = Redirects::None;
    assert_eq!(
        scene.execute().0.failure,
        Some(Failure::Data(DataFailure::RedirectRefused(301)))
    );
    for count in [5, 6] {
        let mut replies = vec![Reply::status(302, "Location: /same\r\n"); count];
        if count == 5 {
            replies.push(Reply::ok(b"ok"));
        }
        let scene = Scene::new(replies);
        assert_eq!(
            scene.execute().0.failure,
            if count == 5 {
                None
            } else {
                Some(Failure::Data(DataFailure::RedirectRefused(302)))
            }
        );
        assert_eq!(scene.requests(), 6);
    }
    for fields in [
        "",
        "Location: /a\r\nLocation: /b\r\n",
        "Location: http://[\r\n",
    ] {
        let scene = Scene::new(vec![Reply::status(302, fields)]);
        assert_eq!(scene.execute().0.failure, download(Detail::Status(302)));
    }
    let scene = Scene::new(vec![Reply::status(
        302,
        &format!("Location: /{}\r\n", "a".repeat(16_384)),
    )]);
    assert_eq!(
        scene.execute().0.failure,
        Some(Failure::Data(DataFailure::RedirectRefused(302)))
    );
    // Location itself fits; joining it adds the scheme and authority.
    let scene = Scene::new(vec![Reply::status(
        302,
        &format!("Location: /{}\r\n", "a".repeat(16_383)),
    )]);
    assert_eq!(
        scene.execute().0.failure,
        Some(Failure::Data(DataFailure::RedirectRefused(302)))
    );
}

#[test]
fn statuses_and_every_retry_class_enabled_disabled_and_attempt_count() {
    for (reply, detail, class) in [
        (
            Reply::fail(Detail::Connect),
            Detail::Connect,
            Some(TryClass::Connect),
        ),
        (
            Reply::fail(Detail::Tls),
            Detail::Tls,
            Some(TryClass::Connect),
        ),
        (
            Reply::fail(Detail::NoResponse),
            Detail::NoResponse,
            Some(TryClass::NoResponse),
        ),
        (
            Reply::fail(Detail::Timeout),
            Detail::Timeout,
            Some(TryClass::Timeout),
        ),
        (
            Reply::raw(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nx".to_vec()),
            Detail::Truncated,
            Some(TryClass::Truncated),
        ),
        (
            Reply::status(408, ""),
            Detail::Status(408),
            Some(TryClass::Timeout),
        ),
        (
            Reply::status(429, ""),
            Detail::Status(429),
            Some(TryClass::Status429),
        ),
        (
            Reply::status(503, ""),
            Detail::Status(503),
            Some(TryClass::Status5xx),
        ),
        (
            Reply::fail(Detail::HeadTooLarge),
            Detail::HeadTooLarge,
            None,
        ),
    ] {
        for enabled in [false, true] {
            let retries = enabled && class.is_some();
            let mut scene = Scene::new(vec![reply.clone(), Reply::ok(b"ok")]);
            scene.policy.attempts = 2;
            scene.policy.retry_on = if enabled {
                class.map_or(RetryOn::ALL, |c| RetryOn::NONE.with(c))
            } else {
                RetryOn::NONE
            };
            let (end, _) = scene.execute();
            assert_eq!(end.failure, if retries { None } else { download(detail) });
            assert_eq!(scene.requests(), if retries { 2 } else { 1 });
        }
    }
    for status in [101, 204, 206, 300, 304, 400, 404] {
        let mut scene = Scene::new(vec![Reply::status(status, "")]);
        scene.policy.attempts = 10;
        assert_eq!(scene.execute().0.failure, download(Detail::Status(status)));
        assert_eq!(scene.requests(), 1);
    }
    let mut scene = Scene::new(vec![Reply::status(500, ""); 10]);
    scene.policy.attempts = 10;
    scene.policy.backoff_initial_ms = 2;
    scene.policy.backoff_max_ms = 9;
    assert_eq!(scene.execute().0.failure, download(Detail::Status(500)));
    assert_eq!(scene.requests(), 10);
    assert_eq!(
        *scene.time.waits.lock().unwrap(),
        [2, 6, 14, 23, 32, 41, 50, 59, 68]
    );
}

#[test]
fn retry_after_largest_seconds_dates_overflow_cap_and_malformed_values() {
    for (fields, expected) in [
        ("Retry-After: 2\r\nRetry-After: 4\r\n", 4000),
        ("Retry-After: bad\r\nRetry-After: -1\r\n", 500),
        ("Retry-After: 999999999999999999999999999999999\r\n", 30_000),
        ("Retry-After: Sun, 06 Nov 1994 08:49:40 GMT\r\n", 3000),
        ("Retry-After: Sun, 06 Nov 1994 08:49:30 GMT\r\n", 500),
        ("Retry-After: 1, 9\r\nRetry-After: 1\r\n", 1000),
    ] {
        let mut scene = Scene::new(vec![Reply::status(503, fields), Reply::ok(b"ok")]);
        scene.policy.attempts = 2;
        assert_eq!(scene.execute().0.failure, None);
        assert_eq!(scene.peer.seen.lock().unwrap()[1].at, expected);
    }
}

#[test]
fn integrity_precedence_reservations_and_facts_nullability() {
    for (supplied, length, body, m, failure, integrity, facts) in [
        (
            Some(9),
            None,
            b"".as_slice(),
            8,
            Some(DataFailure::InputBytes(8)),
            None,
            false,
        ),
        (
            Some(2),
            Some(9),
            b"",
            8,
            Some(DataFailure::IntegrityMismatch),
            Some(Integrity::Size),
            false,
        ),
        (
            None,
            Some(9),
            b"",
            8,
            Some(DataFailure::InputBytes(8)),
            None,
            false,
        ),
        (
            Some(2),
            None,
            b"123456789",
            8,
            Some(DataFailure::InputBytes(8)),
            None,
            false,
        ),
        (
            Some(2),
            None,
            b"123",
            8,
            Some(DataFailure::IntegrityMismatch),
            Some(Integrity::Size),
            false,
        ),
        (
            Some(2),
            None,
            b"1",
            8,
            Some(DataFailure::IntegrityMismatch),
            Some(Integrity::Size),
            true,
        ),
        (
            None,
            None,
            b"12",
            8,
            Some(DataFailure::IntegrityMismatch),
            Some(Integrity::Sha256),
            true,
        ),
    ] {
        let head = length.map_or(String::new(), |n| format!("Content-Length: {n}\r\n"));
        let mut raw = format!("HTTP/1.1 200 OK\r\n{head}\r\n").into_bytes();
        raw.extend_from_slice(body);
        let mut scene = Scene::new(vec![Reply::raw(raw)]);
        if let Some(n) = supplied {
            scene.bundle["size"] = json!(n);
        }
        scene.bundle["sha256"] = json!("0".repeat(64));
        scene.m = m;
        scene.policy.attempts = 10;
        let (end, _) = scene.execute();
        assert_eq!(end.failure, failure.map(Failure::Data));
        assert_eq!(end.integrity, integrity);
        assert_eq!(end.facts.is_some(), facts);
        assert_eq!(scene.counter.used(), 0);
        assert!(scene.requests() <= 1);
        assert_eq!(end.claimed_bytes, 0);
    }
    for supplied in [false, true] {
        let mut scene = Scene::new(vec![Reply::ok(b"ok")]);
        if supplied {
            scene.bundle["size"] = json!(2);
        }
        assert_eq!(scene.execute().0.failure, None);
        assert_eq!(
            scene.peer.seen.lock().unwrap()[0].claimed,
            if supplied { 4096 } else { 0 }
        );
    }
}

#[test]
fn exact_idle_rate_redirect_and_cutoff_boundaries() {
    for (at, expected) in [
        (999, None),
        (1000, download(Detail::Timeout)),
        (1001, download(Detail::Timeout)),
    ] {
        let mut reply = Reply::ok(b"ok");
        reply.body_at = vec![at];
        let mut scene = Scene::new(vec![reply]);
        scene.policy.idle_timeout_ms = 1000;
        assert_eq!(scene.execute().0.failure, expected);
    }
    let mut first = Reply::status(302, "Location: /next\r\n");
    first.head_at = 700;
    let mut second = Reply::ok(b"ok");
    second.head_at = 1000;
    let mut scene = Scene::new(vec![first, second]);
    scene.policy.idle_timeout_ms = 1000;
    assert_eq!(scene.execute().0.failure, download(Detail::Timeout));
    let mut stall = Reply::ok(b"ok");
    stall.stall = true;
    let mut scene = Scene::new(vec![stall]);
    scene.policy.idle_timeout_ms = 1000;
    assert_eq!(scene.execute().0.failure, download(Detail::Timeout));
    let mut scene = Scene::new(vec![Reply::status(503, "")]);
    scene.policy.attempts = 2;
    scene.cutoff = Some(Duration::from_millis(500));
    assert_eq!(scene.execute().0.failure, Some(Failure::Stopped));
    assert_eq!(scene.requests(), 1);
    assert!(scene.time.waits.lock().unwrap().is_empty());
}

#[test]
fn rate_floor_rolls_satisfied_windows_before_crediting_boundary_data() {
    for (floor, expected) in [(1, None), (2, download(Detail::Timeout))] {
        let mut reply = Reply::ok(&vec![b'x'; 4 * unityrs_batch::http::request::PIECE]);
        reply.fragment = 1;
        reply.body_at = vec![999, 1000, 1999, 2000, 2001];
        let mut scene = Scene::new(vec![reply]);
        scene.policy.idle_timeout_ms = 1000;
        scene.policy.min_rate_bytes =
            floor * u32::try_from(unityrs_batch::http::request::PIECE).unwrap();
        assert_eq!(scene.execute().0.failure, expected);
    }
    for (at, expected) in [
        (999, Detail::Connect),
        (1000, Detail::Timeout),
        (1001, Detail::Timeout),
    ] {
        let mut reply = Reply::fail(Detail::Connect);
        reply.head_at = at;
        let mut scene = Scene::new(vec![reply]);
        scene.policy.idle_timeout_ms = 1000;
        assert_eq!(scene.execute().0.failure, download(expected));
    }
}

#[test]
fn content_length_checks_follow_hypers_actual_framing() {
    for fields in [
        "Content-Length: 5, 5\r\n",
        "Content-Length: 5\r\nContent-Length: 5\r\n",
        "Transfer-Encoding: chunked\r\nContent-Length: 99\r\n",
        "Transfer-Encoding: chunked\r\nContent-Length: garbage\r\n",
        "Transfer-Encoding: chunked\r\nContent-Length: 18446744073709551615\r\n",
    ] {
        for supplied in [false, true] {
            let body = if fields.starts_with("Transfer") {
                "5\r\nhello\r\n0\r\n\r\n"
            } else {
                "hello"
            };
            let mut scene = Scene::new(vec![Reply::raw(format!(
                "HTTP/1.1 200 OK\r\n{fields}\r\n{body}"
            ))]);
            scene.m = 5;
            if supplied {
                scene.bundle["size"] = json!(5);
            }
            let (end, stage) = scene.execute();
            assert_eq!(end.failure, None, "{fields}");
            assert_eq!(end.facts.unwrap().size, 5);
            assert_eq!(scene.requests(), 1);
            assert_eq!(
                std::fs::read(stage.file.as_ref().unwrap().path()).unwrap(),
                b"hello"
            );
        }
    }
    for value in ["+5", "5, 6", "", "18446744073709551616"] {
        let scene = Scene::new(vec![Reply::raw(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {value}\r\n\r\nhello"
        ))]);
        assert_eq!(scene.execute().0.failure, download(Detail::NoResponse));
    }
}

#[test]
fn redirect_fragments_userinfo_looking_authority_and_backslashes() {
    for location in ["/next#received_fragment", r"\next#received_fragment"] {
        let scene = Scene::new(vec![
            Reply::status(302, &format!("Location: {location}\r\n")),
            Reply::ok(b"ok"),
        ]);
        assert_eq!(scene.execute().0.failure, None);
        let seen = scene.peer.seen.lock().unwrap();
        assert_eq!(seen[1].uri, "/next");
        assert!(seen.iter().all(|s| !s.uri.contains('#')));
    }
    let scene = Scene::new(vec![Reply::status(302, "Location: #x\r\n"); 6]);
    assert_eq!(
        scene.execute().0.failure,
        Some(Failure::Data(DataFailure::RedirectRefused(302)))
    );
    assert_eq!(scene.requests(), 6);
    assert!(
        scene
            .peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|s| s.uri == "/start?original_secret=hidden")
    );
    for location in [
        "http://source.invalid@other.invalid/",
        r"http:\\other.invalid\x",
    ] {
        let scene = Scene::new(vec![Reply::status(
            302,
            &format!("Location: {location}\r\n"),
        )]);
        assert_eq!(
            scene.execute().0.failure,
            Some(Failure::Data(DataFailure::RedirectRefused(302)))
        );
        assert_eq!(scene.requests(), 1);
    }
}
