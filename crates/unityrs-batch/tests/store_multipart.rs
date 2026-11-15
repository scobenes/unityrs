mod store_support;
use object_store::path::Path;
use std::time::Duration;
use store_support::{Harness, Reply, run};
use unityrs_batch::{
    phase::Account,
    spool::journal::Journal,
    store::{
        multipart::{Target, Transfer, part_count},
        put::{Error, PART_SIZE, Payload, Request},
    },
};
use unityrs_testkit::scratch::Scratch;
fn created(id: &str) -> Reply {
    Reply::status(200,"",format!("<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>").as_bytes())
}
fn part(tag: &str) -> Reply {
    Reply::status(200, &format!("ETag: \"{tag}\"\r\n"), b"")
}
fn complete(tag: &str) -> Reply {
    Reply::status(200,"",format!("<CompleteMultipartUploadResult><Location>http://s3.test/k</Location><Bucket>bucket</Bucket><Key>k</Key><ETag>\"{tag}\"</ETag></CompleteMultipartUploadResult>").as_bytes())
}
#[test]
fn sizes_have_no_empty_last_part_and_stop_before_create_at_ten_thousand() {
    assert_eq!(part_count(PART_SIZE as u64), Ok(1));
    assert_eq!(part_count(PART_SIZE as u64 + 1), Ok(2));
    assert_eq!(part_count(2 * PART_SIZE as u64), Ok(2));
    assert_eq!(part_count(10000 * PART_SIZE as u64), Ok(10000));
    assert_eq!(
        part_count(10000 * PART_SIZE as u64 + 1),
        Err(Error::ArchiveTooLarge)
    );
}
#[test]
fn multipart_journals_before_parts_retires_on_complete_and_releases_phase_memory() {
    let scratch = Scratch::new("multipart");
    let journal = Journal::new(scratch.path()).unwrap();
    let h = Harness::new([
        created("upload"),
        part("one"),
        part("two"),
        complete("expected-2"),
    ]);
    let part_budget = unityrs_batch::store::multipart::PartBudget::new();
    let account = Account::new(32 << 20);
    let key = Path::parse("k").unwrap();
    let result = run(async {
        let tries = h.tries();
        let request = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(4),
        );
        let mut transfer = Transfer::start(
            &request,
            &account,
            &part_budget,
            &journal,
            Target {
                key: &key,
                endpoint: None,
                bucket: "bucket",
            },
            PART_SIZE as u64 + 1,
        )
        .await
        .unwrap();
        assert_eq!(
            journal
                .reader()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .entry()
                .upload_id,
            "upload"
        );
        transfer
            .part(
                &request,
                Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
            )
            .await
            .unwrap();
        transfer
            .part(
                &request,
                Payload::read(&account, 1, &mut &b"z"[..]).unwrap(),
            )
            .await
            .unwrap();
        transfer.complete(&request, b"expected-2").await
    })
    .unwrap();
    assert!(!result.verify_by_size);
    assert!(journal.reader().unwrap().next().unwrap().is_none());
    assert_eq!(h.script.seen.count(), 4);
    assert!(h.script.seen.line(1).contains("partNumber=1"));
    assert!(h.script.seen.line(2).contains("partNumber=2"));
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn oversize_upload_id_is_aborted_once_without_a_journal_entry() {
    let scratch = Scratch::new("multipart-id");
    let journal = Journal::new(scratch.path()).unwrap();
    let mut h = Harness::new([created(&"x".repeat(1025)), Reply::status(204, "", b"")]);
    h.policy.attempts = 3;
    let part_budget = unityrs_batch::store::multipart::PartBudget::new();
    let account = Account::new(32 << 20);
    let key = Path::parse("k").unwrap();
    let result = run(async {
        let tries = h.tries();
        let request = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        Transfer::start(
            &request,
            &account,
            &part_budget,
            &journal,
            Target {
                key: &key,
                endpoint: None,
                bucket: "bucket",
            },
            PART_SIZE as u64 + 1,
        )
        .await
        .map(drop)
    });
    assert_eq!(result, Err(Error::PublishFailed));
    assert_eq!(h.script.seen.count(), 2);
    assert!(journal.reader().unwrap().next().unwrap().is_none());
}

#[test]
fn part_failure_aborts_and_a_failed_abort_keeps_the_durable_id() {
    for abort_ok in [true, false] {
        let scratch = Scratch::new("multipart-part-fault");
        let journal = Journal::new(scratch.path()).unwrap();
        let h = Harness::new([
            created("upload"),
            part(&"x".repeat(101)),
            Reply::error(
                if abort_ok { 404 } else { 403 },
                if abort_ok {
                    "NoSuchUpload"
                } else {
                    "AccessDenied"
                },
            ),
        ]);
        let part_budget = unityrs_batch::store::multipart::PartBudget::new();
        let account = Account::new(32 << 20);
        let key = Path::parse("k").unwrap();
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let mut transfer = Transfer::start(
                &req,
                &account,
                &part_budget,
                &journal,
                Target {
                    key: &key,
                    endpoint: None,
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap();
            assert_eq!(
                transfer
                    .part(
                        &req,
                        Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap()
                    )
                    .await,
                Err(Error::PublishFailed)
            );
        });
        assert_eq!(
            journal.reader().unwrap().next().unwrap().is_none(),
            abort_ok
        );
        assert_eq!(h.unsettled(), 1);
        assert_eq!(h.script.seen.count(), 3);
    }
}
#[test]
fn only_a_retried_completes_no_such_upload_heads_the_expected_key() {
    for matching in [true, false] {
        let scratch = Scratch::new("multipart-complete-retry");
        let journal = Journal::new(scratch.path()).unwrap();
        let mut replies = vec![
            created("upload"),
            part("one"),
            part("two"),
            Reply::error(503, "SlowDown"),
            Reply::error(404, "NoSuchUpload"),
            Reply::status(
                200,
                &format!(
                    "ETag: \"{}\"\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                    if matching { "expected-2" } else { "other" }
                ),
                b"",
            ),
        ];
        if !matching {
            replies.push(Reply::error(404, "NoSuchUpload"));
        }
        let mut h = Harness::new(replies);
        h.policy.attempts = 2;
        let part_budget = unityrs_batch::store::multipart::PartBudget::new();
        let account = Account::new(32 << 20);
        let key = Path::parse("k").unwrap();
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let mut transfer = Transfer::start(
                &req,
                &account,
                &part_budget,
                &journal,
                Target {
                    key: &key,
                    endpoint: None,
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap();
            transfer
                .part(
                    &req,
                    Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
                )
                .await
                .unwrap();
            transfer
                .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                .await
                .unwrap();
            transfer.complete(&req, b"expected-2").await
        });
        if matching {
            assert!(!result.unwrap().verify_by_size);
        } else {
            assert_eq!(result.unwrap_err(), Error::PublishFailed);
        }
        assert!(h.script.seen.line(5).starts_with("HEAD "));
        assert_eq!(h.unsettled(), 1);
        assert!(journal.reader().unwrap().next().unwrap().is_none());
    }
}
#[test]
fn failed_journal_append_aborts_once_and_keeps_the_host_failure() {
    let scratch = Scratch::new("multipart-append-fault");
    let journal = Journal::new(&scratch.join("missing")).unwrap();
    let mut h = Harness::new([created("upload"), Reply::error(503, "SlowDown")]);
    h.policy.attempts = 3;
    let part_budget = unityrs_batch::store::multipart::PartBudget::new();
    let account = Account::new(32 << 20);
    let key = Path::parse("k").unwrap();
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(3),
        );
        Transfer::start(
            &req,
            &account,
            &part_budget,
            &journal,
            Target {
                key: &key,
                endpoint: None,
                bucket: "bucket",
            },
            PART_SIZE as u64 + 1,
        )
        .await
        .map(drop)
    });
    assert!(matches!(result, Err(Error::Phase(_))));
    assert_eq!(h.script.seen.count(), 2);
}

#[test]
fn admission_keeps_part_ids_in_their_separate_two_mib_allowance() {
    let maximum = unityrs_batch::store::multipart::reservation(10000 * PART_SIZE as u64).unwrap();
    assert!(maximum < 40 * 1024 * 1024);
    assert_eq!(
        maximum,
        unityrs_batch::store::multipart::completion_reservation(10000).unwrap()
    );
    assert_eq!(
        unityrs_batch::store::multipart::reservation(PART_SIZE as u64 + 1).unwrap(),
        unityrs_core::limits::heap_cost(PART_SIZE as u64)
    );
    let h = Harness::new([]);
    let scratch = Scratch::new("multipart-refusal");
    let journal = Journal::new(scratch.path()).unwrap();
    let part_budget = unityrs_batch::store::multipart::PartBudget::new();
    let account = Account::new(32 << 20);
    let key = Path::parse("k").unwrap();
    let result = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        Transfer::start(
            &req,
            &account,
            &part_budget,
            &journal,
            Target {
                key: &key,
                endpoint: None,
                bucket: "bucket",
            },
            10000 * PART_SIZE as u64,
        )
        .await
        .map(drop)
    });
    assert!(matches!(result, Err(Error::Phase(_))));
    assert_eq!(h.script.seen.count(), 0);
}

#[test]
fn abort_cleanup_journal_host_failure_overrides_the_publication_failure() {
    for completing in [false, true] {
        let scratch = Scratch::new("multipart-retire-fault");
        let journal = Journal::new(scratch.path()).unwrap();
        let mut replies = vec![created("upload")];
        if completing {
            replies.extend([part("one"), part("two")]);
        }
        replies.extend([
            Reply::error(403, "AccessDenied"),
            Reply::status(204, "", b""),
        ]);
        let h = Harness::new(replies);
        let account = Account::new(32 << 20);
        let budget = unityrs_batch::store::multipart::PartBudget::new();
        let key = Path::parse("k").unwrap();
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let mut transfer = Transfer::start(
                &req,
                &account,
                &budget,
                &journal,
                Target {
                    key: &key,
                    endpoint: None,
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap();
            if completing {
                transfer
                    .part(
                        &req,
                        Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
                    )
                    .await
                    .unwrap();
                transfer
                    .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                    .await
                    .unwrap();
            }
            std::fs::create_dir(scratch.join("journal.tmp")).unwrap();
            if completing {
                transfer.complete(&req, b"expected-2").await.map(drop)
            } else {
                transfer
                    .part(
                        &req,
                        Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
                    )
                    .await
            }
        });
        assert!(
            matches!(result,Err(Error::Phase(unityrs_batch::phase::Error::Run(f))) if f.reason==unityrs_batch::attempt::ReasonCode::HostIo),
            "{result:?}"
        );
    }
}
#[test]
fn failed_recovery_head_is_publish_failed_even_for_expired_credentials() {
    for responses in [
        vec![Reply::error(400, "ExpiredToken")],
        vec![Reply::error(404, "NoSuchKey")],
        vec![
            Reply::status(
                200,
                &format!(
                    "ETag: {}\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                    "x".repeat(101)
                ),
                b"",
            ),
            Reply::status(
                200,
                &format!(
                    "ETag: {}\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                    "x".repeat(101)
                ),
                b"",
            ),
        ],
    ] {
        let scratch = Scratch::new("multipart-head-fault");
        let journal = Journal::new(scratch.path()).unwrap();
        let mut replies = vec![
            created("upload"),
            part("one"),
            part("two"),
            Reply::error(503, "SlowDown"),
            Reply::error(404, "NoSuchUpload"),
        ];
        replies.extend(responses);
        replies.push(Reply::status(204, "", b""));
        let mut h = Harness::new(replies);
        h.policy.attempts = 2;
        let account = Account::new(32 << 20);
        let budget = unityrs_batch::store::multipart::PartBudget::new();
        let key = Path::parse("k").unwrap();
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let mut transfer = Transfer::start(
                &req,
                &account,
                &budget,
                &journal,
                Target {
                    key: &key,
                    endpoint: None,
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap();
            transfer
                .part(
                    &req,
                    Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
                )
                .await
                .unwrap();
            transfer
                .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                .await
                .unwrap();
            transfer.complete(&req, b"expected-2").await
        });
        assert_eq!(result.unwrap_err(), Error::PublishFailed);
    }
}

#[test]
fn completion_reservation_refusal_aborts_before_the_live_deadline() {
    let scratch = Scratch::new("multipart-complete-refusal");
    let journal = Journal::new(scratch.path()).unwrap();
    let h = Harness::new([
        created("upload"),
        part("one"),
        part("two"),
        Reply::status(204, "", b""),
    ]);
    let account = Account::new(32 << 20);
    let budget = unityrs_batch::store::multipart::PartBudget::new();
    let key = Path::parse("k").unwrap();
    run(async {
        let tries = h.tries();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        let req = Request::new(&h.store, &tries, &h.slot, deadline);
        let mut transfer = Transfer::start(
            &req,
            &account,
            &budget,
            &journal,
            Target {
                key: &key,
                endpoint: None,
                bucket: "bucket",
            },
            PART_SIZE as u64 + 1,
        )
        .await
        .unwrap();
        transfer
            .part(
                &req,
                Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
            )
            .await
            .unwrap();
        transfer
            .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
            .await
            .unwrap();
        let occupied =
            unityrs_batch::phase::Buffer::<u8>::new(account.meter(), (32 << 20) - 4096).unwrap();
        assert_eq!(
            transfer.complete(&req, b"expected-2").await,
            Err(Error::Phase(unityrs_batch::phase::Error::internal()))
        );
        assert!(tokio::time::Instant::now() < deadline);
        assert_eq!(h.script.seen.count(), 4);
        assert!(
            h.script
                .seen
                .line(3)
                .starts_with("DELETE /bucket/k?uploadId=upload ")
        );
        assert!(journal.reader().unwrap().next().unwrap().is_none());
        drop(occupied);
    });
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn invalid_part_and_incomplete_completion_abort_the_created_upload() {
    for completing in [false, true] {
        let scratch = Scratch::new("multipart-invalid-state");
        let journal = Journal::new(scratch.path()).unwrap();
        let h = Harness::new([created("upload"), Reply::status(204, "", b"")]);
        let account = Account::new(32 << 20);
        let budget = unityrs_batch::store::multipart::PartBudget::new();
        let key = Path::parse("k").unwrap();
        run(async {
            let tries = h.tries();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            let req = Request::new(&h.store, &tries, &h.slot, deadline);
            let mut transfer = Transfer::start(
                &req,
                &account,
                &budget,
                &journal,
                Target {
                    key: &key,
                    endpoint: None,
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap();
            let result = if completing {
                transfer.complete(&req, b"expected-2").await.map(drop)
            } else {
                transfer
                    .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                    .await
            };
            assert_eq!(
                result,
                Err(Error::Fault(unityrs_batch::store::attempt::Fault::Internal))
            );
            assert!(tokio::time::Instant::now() < deadline);
            assert_eq!(h.script.seen.count(), 2);
            assert!(
                h.script
                    .seen
                    .line(1)
                    .starts_with("DELETE /bucket/k?uploadId=upload ")
            );
            assert!(journal.reader().unwrap().next().unwrap().is_none());
        });
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(budget.used(), 0);
    }
}
