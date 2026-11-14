//! Frozen parser boundaries through staging, retry reset and disk accounting.
mod http_support;
mod stage_support;
mod url_support;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, atomic::Ordering},
};
use unityrs_batch::http::Detail;
use url_support::{
    Reply, Scene,
    disk::{Disk, Fs},
    download,
};

fn check(reply: &Reply, expected: Option<Detail>, body: &[u8]) {
    for attempts in [1, 2] {
        let mut scene = Scene::new(vec![reply.clone(); 2]);
        scene.bundle["size"] = serde_json::json!(body.len());
        scene.policy.attempts = attempts;
        scene.policy.backoff_initial_ms = 0;
        scene.policy.backoff_max_ms = 0;
        let disk = Disk::new(scene.counter.clone(), scene.time.clone());
        let (end, stage) = scene.with_fs(Arc::new(Fs(disk.clone())));
        let requests = if expected.is_some_and(|d| d != Detail::HeadTooLarge) {
            attempts
        } else {
            1
        };
        assert_eq!(end.failure, expected.and_then(download));
        assert_eq!(scene.requests(), requests as usize);
        assert_eq!(end.integrity, None);
        let claim = if expected.is_none() && !body.is_empty() {
            4096
        } else {
            0
        };
        assert_eq!(end.claimed_bytes, claim);
        assert_eq!(scene.counter.used(), claim);
        assert_eq!(scene.peer.live.load(Ordering::Acquire), 0);
        assert!(scene.stop.failures.lock().unwrap().is_empty());
        let file = stage.file.as_ref().unwrap();
        if expected.is_none() {
            let facts = end.facts.unwrap();
            assert_eq!(facts.size, body.len() as u64);
            assert_eq!(
                facts.sha256.as_slice(),
                <sha2::Sha256 as sha2::Digest>::digest(body).as_slice()
            );
            assert_eq!(std::fs::read(file.path()).unwrap(), body);
            assert_eq!(
                std::fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
                0o400
            );
        } else {
            assert_eq!(end.facts, None);
            assert!(!file.path().exists());
        }
        let events = disk.events.lock().unwrap();
        let resets: Vec<_> = events
            .iter()
            .filter(|e| e.operation == "truncate")
            .collect();
        assert_eq!(resets.len(), requests as usize - 1);
        for reset in resets {
            let prefix = if expected == Some(Detail::Truncated) {
                body.len() as u64
            } else {
                0
            };
            assert_eq!(reset.length, prefix);
            assert_eq!(reset.claim, if body.is_empty() { 0 } else { 4096 });
        }
        assert_eq!(
            scene
                .peer
                .seen
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.claimed)
                .collect::<Vec<_>>(),
            vec![if body.is_empty() { 0 } else { 4096 }; requests as usize]
        );
    }
}

#[test]
fn complete_unfinished_fragmented_heads_fields_names_and_extreme_lengths_stage_exactly() {
    for fragment in [1024, usize::MAX] {
        for (size, complete, expected) in [
            (131_071, true, None),
            (131_072, true, None),
            (131_073, true, Some(Detail::HeadTooLarge)),
            (131_071, false, Some(Detail::NoResponse)),
            (131_072, false, Some(Detail::HeadTooLarge)),
        ] {
            let mut reply = Reply::raw(http_support::large_head(size, complete));
            reply.fragment = fragment;
            check(&reply, expected, b"");
        }
    }
    for fields in [100, 101] {
        let raw = format!("HTTP/1.1 200 OK\r\n{}\r\n", "x: y\r\n".repeat(fields));
        check(
            &Reply::raw(raw),
            (fields == 101).then_some(Detail::HeadTooLarge),
            b"",
        );
    }
    for size in [65_535, 65_536] {
        check(
            &Reply::raw(format!(
                "HTTP/1.1 200 OK\r\n{}: x\r\n\r\n",
                "a".repeat(size)
            )),
            (size == 65_536).then_some(Detail::HeadTooLarge),
            b"",
        );
    }
    for (length, expected) in [
        ("18446744073709551614", Detail::HeadTooLarge),
        ("18446744073709551615", Detail::HeadTooLarge),
        ("18446744073709551616", Detail::NoResponse),
        ("-1", Detail::NoResponse),
    ] {
        check(
            &Reply::raw(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n"
            )),
            Some(expected),
            b"",
        );
    }
    // Largest nonreserved length parses, then staging's M check refuses it.
    let mut scene = Scene::new(vec![Reply::raw(
        "HTTP/1.1 200 OK\r\nContent-Length: 18446744073709551613\r\n\r\n",
    )]);
    scene.policy.attempts = 2;
    let (end, _) = scene.execute();
    assert_eq!(
        end.failure,
        Some(unityrs_batch::stage::Failure::Data(
            unityrs_batch::stage::DataFailure::InputBytes(scene.m)
        ))
    );
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    assert_eq!(scene.requests(), 1);
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn informational_head_boundaries_malformed_fields_and_no_body_credit_stage_exactly() {
    for fragment in [1024, usize::MAX] {
        for (size, complete, expected) in [
            (131_071, true, None),
            (131_072, true, None),
            (131_073, true, Some(Detail::HeadTooLarge)),
            (131_072, false, Some(Detail::HeadTooLarge)),
        ] {
            let mut raw = http_support::large_head(size, complete);
            raw[9..12].copy_from_slice(b"103");
            if complete {
                raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
            let mut reply = Reply::raw(raw);
            reply.fragment = fragment;
            check(&reply, expected, b"");
        }
    }
    check(
        &Reply::raw(
            "HTTP/1.1 103 Early Hints\r\ninvalid\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        ),
        Some(Detail::NoResponse),
        b"",
    );
    let informational = b"HTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\n";
    let mut raw = informational.repeat(2);
    raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    let mut reply = Reply::raw(raw);
    reply.read_at = vec![(0, 999), (informational.len(), 1001)];
    let mut scene = Scene::new(vec![reply]);
    scene.policy.idle_timeout_ms = 1000;
    let (end, _) = scene.execute();
    assert_eq!(end.failure, download(Detail::Timeout));
    assert_eq!(
        (end.facts, end.integrity, end.claimed_bytes),
        (None, None, 0)
    );
    assert_eq!(scene.requests(), 1);
    assert_eq!(scene.counter.used(), 0);
}

#[test]
fn trailer_byte_equality_above_boundary_fields_and_near_cap_stage_exactly() {
    for fragment in [1024, usize::MAX] {
        for size in [131_071, 131_072, 131_073] {
            let mut raw =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nx: "
                    .to_vec();
            // hyper's trailer accounting includes the zero chunk's CRLF.
            let start = raw.len() - 3;
            raw.resize(start + size - 4, b'a');
            raw.extend_from_slice(b"\r\n\r\n");
            let mut reply = Reply::raw(raw);
            reply.fragment = fragment;
            check(
                &reply,
                (size >= 131_072).then_some(Detail::Truncated),
                b"abc",
            );
        }
        for fields in [100, 101] {
            let mut reply = Reply::raw(format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n{}\r\n",
                "x: y\r\n".repeat(fields)
            ));
            reply.fragment = fragment;
            check(&reply, (fields == 101).then_some(Detail::Truncated), b"abc");
        }
    }
}

#[test]
fn reset_before_status_partial_status_and_malformed_heads_retry_through_staging() {
    for raw in [
        b"HTTP/1.1 200 OK\r\n\r\n".as_slice(),
        b"HTTP/1.1 200",
        b"HTTP/1.1 200 OK\r\ninvalid\r\n\r\n",
    ] {
        let mut reply = Reply::raw(raw.to_vec());
        if raw.ends_with(b"OK\r\n\r\n") {
            reply.reset_at = Some(0);
        }
        check(&reply, Some(Detail::NoResponse), b"");
    }
}
