//! Library operations use B11a's one-request tries; no run integration.
mod store_support;
use hyper::body::Bytes;
use object_store::path::Path;
use std::time::Duration;
use store_support::{Harness, Reply, run};
use tokio::time::Instant;
use unityrs_batch::{
    phase::Account,
    store::{
        head,
        put::{self, Error, Request},
    },
};

fn reply() -> Reply {
    Reply::status(200, "ETag: \"ABC\"\r\n", b"")
}

#[test]
fn single_put_sets_type_compares_etag_and_keeps_one_request_per_try() {
    let mut h = Harness::new([Reply::error(503, "SlowDown"), reply()]);
    h.policy.attempts = 2;
    let account = Account::new(32 << 20);
    let payload = put::Payload::read(&account, 3, &mut &b"png"[..]).unwrap();
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            Instant::now() + Duration::from_secs(2),
        );
        req.put(
            &Path::parse("out/a.png").unwrap(),
            payload,
            "image/png",
            b"abc",
        )
        .await
    })
    .unwrap();
    assert!(!result.verify_by_size);
    assert_eq!(result.etag.as_bytes(), b"\"ABC\"");
    assert_eq!(h.script.seen.count(), 2);
    assert_eq!(h.unsettled(), 1);
    assert_eq!(
        h.script.seen.header(0, "content-type").as_deref(),
        Some("image/png")
    );
    assert_eq!(account.snapshot().input.used, 0);
    assert!(h.slot.take_permit().is_some());
}

#[test]
fn different_etag_is_written_for_size_verification_and_oversize_is_rejected() {
    for (tag, expected) in [("opaque".to_owned(), true), ("x".repeat(101), false)] {
        let h = Harness::new([Reply::status(200, &format!("ETag: {tag}\r\n"), b"")]);
        let account = Account::new(1024);
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                Instant::now() + Duration::from_secs(2),
            );
            req.put(
                &Path::parse("k").unwrap(),
                put::Payload::read(&account, 1, &mut &b"x"[..]).unwrap(),
                "application/json",
                b"abc",
            )
            .await
        });
        if expected {
            assert!(result.unwrap().verify_by_size);
        } else {
            assert_eq!(result.unwrap_err(), Error::PublishFailed);
            assert_eq!(h.unsettled(), 1);
        }
    }
}

#[test]
fn expired_deadline_sends_nothing_and_buffer_refusal_precedes_read() {
    let h = Harness::new([]);
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            Instant::now() - Duration::from_secs(1),
        );
        req.bytes(
            &Path::parse("k").unwrap(),
            Bytes::from_static(b"x"),
            "image/png",
            b"abc",
        )
        .await
    });
    assert_eq!(result.unwrap_err(), Error::Stopped);
    assert_eq!(h.script.seen.count(), 0);
    let account = Account::new(32);
    assert!(put::Payload::read(&account, 33, &mut &b""[..]).is_err());
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn bucket_proof_uses_any_2xx_and_drops_unparsed_body() {
    let h = Harness::new([Reply::status(200, "", b"not xml")]);
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            Instant::now() + Duration::from_secs(2),
        );
        assert_eq!(head::prove(&req, "prefix/").await, Ok(()));
    });
    assert!(h.script.seen.line(0).contains("max-keys=1"));
}

fn identity(attempt: u32, seq: u64) -> Vec<u8> {
    format!(r#"{{"manifest_version":1,"run_id":"run","attempt":{attempt},"write_seq":{seq},"binary_version":"test","spec_version":1,"spec_sha256":"{}"}}"#,"0".repeat(64)).into_bytes()
}
fn get(body: &[u8], tag: &str) -> Reply {
    Reply::status(
        200,
        &format!("ETag: {tag}\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n"),
        body,
    )
}
fn fenced(
    h: &Harness,
    attempt: u32,
    seq: u64,
    initial: Option<unityrs_batch::store::etag::Etag>,
) -> (
    Result<unityrs_batch::store::etag::Etag, head::Error>,
    head::Fence,
) {
    let fence = head::Fence::new(initial);
    let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            Instant::now() + Duration::from_secs(3),
        );
        head::Write {
            key: &Path::parse("_unityrs/run/manifest.json").unwrap(),
            run_id: "run",
            identity: unityrs_batch::manifest::head::Identity {
                attempt,
                write_seq: seq,
            },
            bytes: Bytes::from(identity(attempt, seq)),
            conditional: true,
        }
        .publish(&req, &fence, &mut buffer)
        .await
    });
    (result, fence)
}
#[test]
fn every_conditional_status_reads_back_then_updates_with_the_observed_tag() {
    for status in [304, 409, 412, 404] {
        let initial =
            (status == 404).then(|| unityrs_batch::store::etag::Etag::new(b"\"old\"").unwrap());
        let h = Harness::new([
            Reply::error(status, "PreconditionFailed"),
            get(&identity(1, 0), "\"prior\""),
            reply(),
        ]);
        let (result, fence) = fenced(&h, 2, 0, initial);
        assert!(result.is_ok(), "{status}: {result:?}");
        assert_eq!(fence.last().unwrap().as_bytes(), b"\"prior\"");
        assert_eq!(
            h.script.seen.header(2, "if-match").as_deref(),
            Some("\"prior\"")
        );
        assert_eq!(h.script.seen.line(1).split(' ').next(), Some("GET"));
        assert_eq!(h.unsettled(), 0);
    }
}
#[test]
fn read_back_recovers_lost_write_and_refuses_newer_or_unreadable_heads() {
    for seq in [0, 1] {
        for (body, expected) in [
            (identity(2, seq), None),
            (identity(3, 0), Some(head::Error::Superseded)),
            (
                b"{}".to_vec(),
                Some(if seq == 0 {
                    head::Error::ManifestUnreadable
                } else {
                    head::Error::PublishFailed
                }),
            ),
            (
                vec![b'x'; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap() + 100],
                Some(if seq == 0 {
                    head::Error::ManifestUnreadable
                } else {
                    head::Error::PublishFailed
                }),
            ),
        ] {
            let h = Harness::new([
                Reply::error(412, "PreconditionFailed"),
                get(&body, "\"stored\""),
            ]);
            let (result, _) = fenced(&h, 2, seq, None);
            match expected {
                None => assert_eq!(result.unwrap().as_bytes(), b"\"stored\""),
                Some(e) => assert_eq!(result.unwrap_err(), e),
            }
            assert_eq!(h.script.seen.count(), 2);
        }
    }
}
#[test]
fn vanished_head_uses_create_and_the_fifth_conflict_stops_rewriting() {
    let h = Harness::new([
        Reply::error(409, "ConditionalRequestConflict"),
        Reply::error(404, "NoSuchKey"),
        reply(),
    ]);
    assert!(fenced(&h, 2, 0, None).0.is_ok());
    assert_eq!(
        h.script.seen.header(2, "if-none-match").as_deref(),
        Some("*")
    );
    for seq in [0, 1] {
        let replies = (0..5).flat_map(|_| {
            [
                Reply::error(412, "PreconditionFailed"),
                get(&identity(1, 0), "\"old\""),
            ]
        });
        let h = Harness::new(replies);
        assert_eq!(
            fenced(&h, 2, seq, None).0.unwrap_err(),
            if seq == 0 {
                head::Error::PreviousAttemptLive
            } else {
                head::Error::PublishFailed
            }
        );
        assert_eq!(h.script.seen.count(), 10);
    }
}
#[test]
fn preflight_proves_bucket_before_reading_and_supersedes_equal_attempt() {
    for status in [404, 403, 503] {
        let h = Harness::new([Reply::error(status, "NoSuchBucket")]);
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                Instant::now() + Duration::from_secs(2),
            );
            head::preflight(
                &req,
                "p/",
                &Path::parse("head").unwrap(),
                "run",
                2,
                &mut vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()],
            )
            .await
        });
        assert_eq!(
            result.unwrap_err(),
            if status == 404 {
                head::Error::BucketMissing
            } else {
                head::Error::SinkUnreachable
            }
        );
        assert_eq!(h.script.seen.count(), 1);
    }
    let h = Harness::new([
        Reply::status(200, "", b"<Error>"),
        get(&identity(2, 0), "\"head\""),
    ]);
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            Instant::now() + Duration::from_secs(2),
        );
        head::preflight(
            &req,
            "p/",
            &Path::parse("head").unwrap(),
            "run",
            2,
            &mut vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()],
        )
        .await
    });
    assert_eq!(result.unwrap_err(), head::Error::Superseded);
}
