//! A store request's tries over an in-memory transport: each try's row of
//! `docs/BATCH_SPEC.md` 1.6's table as the connector records it, the
//! unsettled count, retries, the stop and the cutoff, and the connector's
//! body caps.
mod store_support;
use futures_util::StreamExt;
use object_store::{
    ObjectStoreExt, PutPayload, list::PaginatedListOptions, list::PaginatedListStore,
    multipart::MultipartStore, path::Path,
};
use std::time::Duration;
use store_support::{End, Harness, Reply};
use unityrs_batch::{
    http::Detail,
    store::{
        Kind,
        attempt::{BodyFault, Done, Fault, Record},
        classify::{Action, Ended, Verdict},
    },
};

const TIB: u64 = 1 << 40;

#[allow(clippy::unnecessary_wraps)] // Compared with `Done::verdict`.
fn verdict(settled: bool, action: Action) -> Option<Verdict> {
    Some(Verdict { settled, action })
}
const RETRY: Action = Action::Retry { retry_after: true };
const RETRY_PLAIN: Action = Action::Retry { retry_after: false };

fn put(h: &Harness) -> (Option<Verdict>, Record, u32) {
    let path = Path::parse("out/a.png").unwrap();
    let done = h
        .run(Kind::Put, |_| {
            h.store.put(&path, PutPayload::from_static(b"png"))
        })
        .unwrap();
    (done.verdict, done.record, done.tries)
}

fn list(h: &Harness, kind: Kind) -> (Option<Verdict>, Record) {
    let done = h
        .run(kind, |_| {
            h.store.list_paginated(
                Some("out/"),
                PaginatedListOptions {
                    max_keys: Some(1_000),
                    ..PaginatedListOptions::default()
                },
            )
        })
        .unwrap();
    (done.verdict, done.record)
}

fn ok() -> Reply {
    Reply::status(200, "ETag: \"e\"\r\n", b"")
}

#[test]
fn a_connect_failure_is_settled_and_retried() {
    let mut h = Harness::new([
        Reply::Refuse(Detail::Connect),
        Reply::Refuse(Detail::Tls),
        ok(),
    ]);
    h.policy.attempts = 3;
    let (v, record, tries) = put(&h);
    assert_eq!((v, tries), (verdict(true, Action::Success), 3));
    assert_eq!(record.status, Some(200));
    assert_eq!(h.unsettled(), 0);
    let mut h = Harness::new([Reply::Refuse(Detail::Tls)]);
    h.policy.attempts = 1;
    let (v, record, _) = put(&h);
    assert_eq!(v, verdict(true, RETRY_PLAIN));
    assert_eq!(
        (record.connected, record.detail),
        (false, Some(Detail::Tls))
    );
}

#[test]
fn the_timer_during_a_connect_is_settled_and_after_it_is_not() {
    let mut h = Harness::new([Reply::StallConnect]);
    h.policy.timeout = Duration::from_millis(20);
    let (v, record, _) = put(&h);
    assert_eq!(v, verdict(true, RETRY_PLAIN));
    assert!(!record.connected);
    assert_eq!(h.unsettled(), 0);
    let mut h = Harness::new([
        Reply::chunks(vec![], End::Stall),
        Reply::chunks(vec![], End::Stall),
    ]);
    h.policy.timeout = Duration::from_millis(20);
    h.policy.attempts = 2;
    let (v, record, tries) = put(&h);
    assert_eq!((v, tries), (verdict(false, RETRY_PLAIN), 2));
    assert!(record.connected);
    assert_eq!(record.status, None);
    // Both tries may still commit: each stays counted.
    assert_eq!(h.unsettled(), 2);
    // The permit came back from the dropped call.
    assert!(h.slot.take_permit().is_some());
}

#[test]
fn a_dropped_connection_before_the_status_is_unsettled() {
    let h = Harness::new([Reply::chunks(vec![b"HTTP/1.1 200".to_vec()], End::Reset)]);
    let (v, record, _) = put(&h);
    assert_eq!(v, verdict(false, RETRY_PLAIN));
    assert_eq!(record.detail, Some(Detail::NoResponse));
    assert_eq!(h.unsettled(), 1);
}

#[test]
fn statuses_take_their_rows() {
    let cases = [
        (Reply::error(503, "SlowDown"), verdict(false, RETRY)),
        (Reply::error(429, "SlowDown"), verdict(true, RETRY)),
        (Reply::error(408, "RequestTimeout"), verdict(true, RETRY)),
        (
            Reply::error(400, "ExpiredToken"),
            verdict(true, Action::CredentialsExpired),
        ),
        (
            Reply::error(403, "AccessDenied"),
            verdict(true, Action::PublishFailed),
        ),
        (
            Reply::error(404, "NoSuchBucket"),
            verdict(true, Action::PublishFailed),
        ),
        (
            Reply::status(301, "Location: https://elsewhere.test/\r\n", b""),
            verdict(true, Action::PublishFailed),
        ),
        (
            Reply::status(307, "", b""),
            verdict(true, Action::PublishFailed),
        ),
        (ok(), verdict(true, Action::Success)),
    ];
    for (reply, expected) in cases {
        let mut h = Harness::new([reply]);
        h.policy.attempts = 1;
        let (v, record, tries) = put(&h);
        assert_eq!((v, tries), (expected, 1), "{record:?}");
        assert_eq!(h.unsettled(), u64::from(!expected.unwrap().settled));
    }
}

#[test]
fn retries_back_off_raised_by_retry_after_and_stop_at_the_attempts() {
    let mut h = Harness::new([
        Reply::status(503, "Retry-After: 1\r\n", b""),
        Reply::status(503, "", b""),
        Reply::status(503, "", b""),
    ]);
    h.policy.attempts = 3;
    h.policy.backoff_initial_ms = 1;
    h.policy.backoff_max_ms = 40;
    let start = std::time::Instant::now();
    let (v, _, tries) = put(&h);
    // Retry-After's 1 s is capped at backoff_max_ms.
    assert!(start.elapsed() >= Duration::from_millis(40));
    assert_eq!((v, tries), (verdict(false, RETRY), 3));
    assert_eq!(h.script.sent.load(std::sync::atomic::Ordering::Acquire), 3);
    assert_eq!(h.unsettled(), 3);
    // ExpiredToken is never retried, whatever tries remain.
    let mut h = Harness::new([Reply::error(400, "ExpiredToken"), ok()]);
    h.policy.attempts = 3;
    assert_eq!(put(&h).2, 1);
}

#[test]
fn no_try_starts_after_a_stop_or_past_the_cutoff() {
    let h = Harness::new([ok()]);
    h.stop.store(true, std::sync::atomic::Ordering::Release);
    let path = Path::parse("k").unwrap();
    let done = h
        .run(Kind::Put, |_| {
            h.store.put(&path, PutPayload::from_static(b"x"))
        })
        .unwrap();
    assert!(done.stopped && done.verdict.is_none() && done.tries == 0);
    assert_eq!(h.script.sent.load(std::sync::atomic::Ordering::Acquire), 0);
    // A retry whose wait would pass the cutoff never starts.
    let mut h = Harness::new([Reply::error(503, "SlowDown"), ok()]);
    h.policy.attempts = 3;
    h.policy.backoff_initial_ms = 60_000;
    h.policy.backoff_max_ms = 60_000;
    h.policy.cutoff = Some(tokio::time::Instant::now() + Duration::from_secs(5));
    let (v, _, tries) = put(&h);
    assert_eq!((v, tries), (verdict(false, RETRY), 1));
}

#[test]
fn head_writes_and_reads_take_their_own_rows() {
    for (status, if_match, expected) in [
        (412, true, Action::ReadBack),
        (409, false, Action::ReadBack),
        (304, true, Action::ReadBack),
        (404, true, Action::ReadBack),
        (404, false, Action::PublishFailed),
    ] {
        let h = Harness::new([Reply::status(status, "", b"")]);
        let path = Path::parse("_unityrs/r/manifest.json").unwrap();
        let done = h
            .run(Kind::HeadWrite { if_match }, |_| {
                h.store.put(&path, PutPayload::from_static(b"{}"))
            })
            .unwrap();
        assert_eq!(done.verdict, verdict(true, expected), "{status} {if_match}");
        assert_eq!(h.unsettled(), 0);
    }
    let h = Harness::new([Reply::error(404, "NoSuchKey")]);
    let path = Path::parse("_unityrs/r/manifest.json").unwrap();
    let done = h.run(Kind::HeadRead, |_| h.store.get(&path)).unwrap();
    assert_eq!(done.verdict, verdict(true, Action::NoHead));
}

#[test]
fn a_retried_complete_meeting_no_such_upload_checks_the_head() {
    let mut h = Harness::new([
        Reply::chunks(vec![], End::Reset),
        Reply::error(404, "NoSuchUpload"),
    ]);
    h.policy.attempts = 2;
    let path = Path::parse("a.zip").unwrap();
    let upload = "upload".to_owned();
    let done = h
        .run(Kind::Complete, |_| {
            h.store.complete_multipart(
                &path,
                &upload,
                vec![object_store::multipart::PartId {
                    content_id: "\"p1\"".to_owned(),
                }],
            )
        })
        .unwrap();
    assert_eq!(
        (done.verdict, done.tries),
        (verdict(true, Action::HeadCheck), 2)
    );
    // The first try's dropped connection stays counted.
    assert_eq!(h.unsettled(), 1);
    // An abort's NoSuchUpload is a successful abort.
    let h = Harness::new([Reply::error(404, "NoSuchUpload")]);
    let done = h
        .run(Kind::Abort, |_| h.store.abort_multipart(&path, &upload))
        .unwrap();
    assert_eq!(done.verdict, verdict(true, Action::Success));
}

#[test]
fn a_two_chunk_body_under_a_huge_content_length_fails_by_its_status_row() {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {TIB}\r\n\r\n<ListBucketResult>");
    let h = Harness::new([Reply::chunks(
        vec![head.into_bytes(), b"<Name>bucket</Name>".to_vec()],
        End::Eof,
    )]);
    let (v, record) = list(&h, Kind::ListPage);
    assert_eq!(v, verdict(false, RETRY));
    assert_eq!(record.body, Some(BodyFault::Transport(Detail::Truncated)));
    let head = format!("HTTP/1.1 503 X\r\nContent-Length: {TIB}\r\n\r\n<Error><Code>Slow");
    let h = Harness::new([Reply::chunks(
        vec![head.into_bytes(), b"Down</Code>".to_vec()],
        End::Eof,
    )]);
    let (v, record) = list(&h, Kind::ListPage);
    assert_eq!(v, verdict(false, RETRY));
    assert!(record.code.is("SlowDown"));
    assert_eq!(record.body, Some(BodyFault::Transport(Detail::Truncated)));
}

#[test]
fn a_client_error_body_over_64_kib_is_cut_and_its_status_decides() {
    let mut body = b"<Error><Code>ExpiredToken</Code>".to_vec();
    body.resize(100_000, b' ');
    let h = Harness::new([Reply::status(400, "", &body)]);
    let (v, record, _) = put(&h);
    assert_eq!(v, verdict(true, Action::CredentialsExpired));
    assert_eq!(record.body, None);
    let mut body = vec![b' '; 70_000];
    body.extend_from_slice(b"<Code>ExpiredToken</Code>");
    let h = Harness::new([Reply::status(400, "", &body)]);
    let (v, record, _) = put(&h);
    // The code lay past the cut: no code, any other 4xx.
    assert_eq!(v, verdict(true, Action::PublishFailed));
    assert!(!record.code.is("ExpiredToken"));
}

#[test]
fn a_2xx_body_past_its_cap_fails_and_a_heads_read_stops() {
    let body = vec![b'x'; 70_000];
    let h = Harness::new([Reply::status(200, "", &body)]);
    let path = Path::parse("a.zip").unwrap();
    let done = h
        .run(Kind::Create, |_| h.store.create_multipart(&path))
        .unwrap();
    assert_eq!(done.verdict, verdict(false, RETRY));
    assert_eq!(done.record.body, Some(BodyFault::OverCap));
    assert_eq!(h.unsettled(), 1);
    let head = vec![b'{'; 2 * 1_024 * 1_024];
    let h = Harness::new([Reply::status(200, "", &head)]);
    let path = Path::parse("_unityrs/r/manifest.json").unwrap();
    let done = h
        .run(Kind::HeadRead, |_| {
            let path = &path;
            let store = &h.store;
            async move {
                let mut stream = store.get(path).await?.into_stream();
                let mut read = 0_usize;
                while let Some(chunk) = stream.next().await {
                    read += chunk?.len();
                }
                Ok(read)
            }
        })
        .unwrap();
    assert_eq!(done.value, Some(1_024 * 1_024 + 1));
    assert_eq!(done.verdict, verdict(true, Action::Success));
}

#[test]
fn a_list_page_breaking_the_tag_scan_fails_and_the_proof_takes_any_2xx() {
    let mut deep = String::from("<ListBucketResult>");
    for _ in 0..8 {
        deep.push_str("<a>");
    }
    let h = Harness::new([Reply::status(200, "", deep.as_bytes())]);
    let (v, record) = list(&h, Kind::ListPage);
    assert_eq!(v, verdict(false, RETRY));
    assert_eq!(record.body, Some(BodyFault::Scan));
    // The same page, as the bucket proof: its body is dropped unread.
    let h = Harness::new([Reply::status(200, "", deep.as_bytes())]);
    assert_eq!(list(&h, Kind::ListProof).0, verdict(true, Action::Success));
    let h = Harness::new([Reply::error(404, "NoSuchBucket")]);
    assert_eq!(
        list(&h, Kind::ListProof).0,
        verdict(true, Action::PublishFailed)
    );
    // A page within the limits parses.
    let page = "<ListBucketResult><Name>bucket</Name><Prefix>out/</Prefix><KeyCount>1</KeyCount>\
        <MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated><Contents><Key>out/a.png</Key>\
        <LastModified>2026-11-01T00:00:00.000Z</LastModified><ETag>&quot;e&quot;</ETag>\
        <Size>3</Size><StorageClass>STANDARD</StorageClass></Contents></ListBucketResult>";
    let h = Harness::new([Reply::status(200, "", page.as_bytes())]);
    let path = Path::parse("x").unwrap();
    let _ = path;
    assert_eq!(list(&h, Kind::ListPage).0, verdict(true, Action::Success));
}

#[test]
fn a_2xx_naming_an_error_is_retried() {
    let body = b"<Error><Code>InternalError</Code><Message>m</Message></Error>";
    let h = Harness::new([Reply::status(200, "", body)]);
    let path = Path::parse("a.zip").unwrap();
    let upload = "upload".to_owned();
    let done = h
        .run(Kind::Complete, |_| {
            h.store.complete_multipart(
                &path,
                &upload,
                vec![object_store::multipart::PartId {
                    content_id: "\"p1\"".to_owned(),
                }],
            )
        })
        .unwrap();
    assert_eq!(done.verdict, verdict(false, RETRY));
    assert!(done.record.code.is("InternalError"));
}

#[test]
fn a_try_that_sent_no_request_is_internal_whatever_it_ended() {
    // Review round 1: never a connect failure, never retried.
    for ended in [Ended::Ok, Ended::Rejected, Ended::Error, Ended::TimedOut] {
        let mut h = Harness::new([]);
        h.policy.attempts = 3;
        h.policy.timeout = Duration::from_millis(10);
        let tries = std::cell::Cell::new(0);
        let done = store_support::run(h.tries().run(&h.slot, Kind::Put, |_| {
            tries.set(tries.get() + 1);
            async move {
                match ended {
                    Ended::Ok => Ok(()),
                    // The try's timer fires before any request goes out.
                    Ended::TimedOut => std::future::pending().await,
                    other => Err(other),
                }
            }
        }));
        assert_eq!(done.unwrap_err(), Fault::Internal, "{ended:?}");
        assert_eq!(tries.get(), 1, "{ended:?}");
        assert_eq!(h.script.seen.count(), 0);
    }
}

#[test]
fn a_slow_lookup_holds_its_token_outside_later_tries_timers() {
    // Review round 1: the lookup a timed-out try left running holds the
    // permit's token past several timers; the next try waits for it
    // untimed and uncounted, then succeeds.
    let mut h = Harness::new([Reply::SlowLookup(Duration::from_millis(300)), ok()]);
    h.policy.attempts = 2;
    h.policy.timeout = Duration::from_millis(30);
    let (v, record, tries) = put(&h);
    assert_eq!(v, verdict(true, Action::Success));
    assert_eq!(record.status, Some(200));
    assert_eq!(tries, 2);
    // Past the cutoff, the wait ends the tries as stopped, starting none.
    let mut h = Harness::new([Reply::SlowLookup(Duration::from_secs(60)), ok()]);
    h.policy.attempts = 2;
    h.policy.timeout = Duration::from_millis(30);
    h.policy.cutoff = Some(tokio::time::Instant::now() + Duration::from_millis(200));
    let path = Path::parse("out/a.png").unwrap();
    let done = h
        .run(Kind::Put, |_| {
            h.store.put(&path, PutPayload::from_static(b"png"))
        })
        .unwrap();
    assert!(done.stopped);
    assert_eq!(done.tries, 1);
    assert_eq!(h.script.sent.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn a_2xx_body_read_after_its_try_is_refused() {
    // Review round 1: a body only its own try's timer bounds.
    let h = Harness::new([Reply::status(200, "", b"head")]);
    let path = Path::parse("_unityrs/r/manifest.json").unwrap();
    let done = h.run(Kind::HeadRead, |_| h.store.get(&path)).unwrap();
    assert_eq!(done.verdict, verdict(true, Action::Success));
    let mut stream = done.value.unwrap().into_stream();
    let read = store_support::run(async move { stream.next().await });
    assert!(matches!(read, Some(Err(_))));
    assert_eq!(h.slot.record().fault, Some(Fault::Internal));
}

/// A complete answered with `body`, its bytes cut every `size` bytes and at
/// each of `cuts` (offsets in the body).
fn complete(body: &[u8], size: usize, cuts: &[usize]) -> Done<object_store::PutResult> {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    let mut bytes = head.clone().into_bytes();
    bytes.extend_from_slice(body);
    let mut at: Vec<usize> = (1..bytes.len()).filter(|i| i % size == 0).collect();
    at.extend(cuts.iter().map(|c| head.len() + c));
    at.sort_unstable();
    at.dedup();
    let mut chunks = Vec::new();
    let mut from = 0;
    for to in at.into_iter().chain([bytes.len()]) {
        chunks.push(bytes[from..to].to_vec());
        from = to;
    }
    let h = Harness::new([Reply::chunks(chunks, End::Eof)]);
    let path = Path::parse("out/SlowDown.zip").unwrap();
    let upload = "upload".to_owned();
    h.run(Kind::Complete, |_| {
        h.store.complete_multipart(
            &path,
            &upload,
            vec![object_store::multipart::PartId {
                content_id: "\"p1\"".to_owned(),
            }],
        )
    })
    .unwrap()
}

#[test]
fn a_complete_whose_answer_holds_an_error_word_succeeds() {
    // Review round 2: the root decides, and `object_store`'s substring
    // check never sees the words in the answer's text.
    for (key, etag) in [
        ("out/SlowDown.zip", "\"InternalError-1\""),
        ("out/InternalError.zip", "\"SlowDownSlowDown\""),
        ("out/a.zip", "&quot;SlowDown&quot;"),
    ] {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<CompleteMultipartUploadResult>\
             <Location>https://s3.example.test/bucket/{key}</Location><Bucket>bucket</Bucket>\
             <Key>{key}</Key><ETag>{etag}</ETag></CompleteMultipartUploadResult>"
        );
        let words: Vec<usize> = ["SlowDown", "InternalError"]
            .iter()
            .flat_map(|w| body.match_indices(w).map(|(i, _)| i + 3))
            .collect();
        for size in [1, 2, 5, 7, body.len() + 64] {
            let done = complete(body.as_bytes(), size, &words);
            assert_eq!(done.verdict, verdict(true, Action::Success), "{key} {size}");
            let want = etag.replace("&quot;", "\"");
            assert_eq!(done.value.unwrap().e_tag.as_deref(), Some(want.as_str()));
        }
    }
}

#[test]
fn a_complete_answered_by_an_error_root_fails() {
    for body in [
        &b"<Error><Code>InternalError</Code><Message>m</Message></Error>"[..],
        b"<?xml version=\"1.0\"?><!-- <CompleteMultipartUploadResult> --><Error><Code>SlowDown</Code></Error>",
        // A word in markup would reach `object_store`'s check as sent.
        b"<CompleteMultipartUploadResult><ETag>\"e\"</ETag><!-- SlowDown --></CompleteMultipartUploadResult>",
    ] {
        for size in [1, 3, 64] {
            let done = complete(body, size, &[]);
            assert_eq!(done.verdict, verdict(false, RETRY), "{}", String::from_utf8_lossy(body));
            assert!(done.value.is_none());
        }
    }
    let done = complete(b"<Error><Code>InternalError</Code></Error>", 4, &[]);
    assert!(done.record.code.is("InternalError"));
    // Past the cap, as before.
    let done = complete(&vec![b' '; 70_000], 4_096, &[]);
    assert_eq!(done.record.body, Some(BodyFault::OverCap));
}

#[test]
fn a_complete_with_a_doctype_is_read_as_object_store_reads_it() {
    // Review round 3: what a DOCTYPE's subset holds stays in it, as
    // quick-xml's parse, `object_store`'s, reads it.
    let answer =
        "<CompleteMultipartUploadResult><ETag>\"e\"</ETag></CompleteMultipartUploadResult>";
    for doctype in [
        "<!DOCTYPE CompleteMultipartUploadResult [<!-- ]><Error> -->]>",
        "<!DOCTYPE r [<?pi ]><Error> ?>]>",
        "<!DOCTYPE r [<!ENTITY q \"]><Error>\">]>",
        "<!DOCTYPE CompleteMultipartUploadResult>",
    ] {
        let body = format!("{doctype}{answer}");
        for size in [1, 2, 5, 7, body.len() + 64] {
            let done = complete(body.as_bytes(), size, &[]);
            assert_eq!(
                done.verdict,
                verdict(true, Action::Success),
                "{doctype} {size}"
            );
            assert_eq!(done.value.unwrap().e_tag.as_deref(), Some("\"e\""));
        }
    }
    let body = b"<!DOCTYPE Error [<!-- <CompleteMultipartUploadResult> -->]><Error><Code>InternalError</Code></Error>";
    for size in [1, 4, 64] {
        let done = complete(body, size, &[]);
        assert_eq!(done.verdict, verdict(false, RETRY));
        assert!(done.record.code.is("InternalError"));
    }
}
