//! Sweep the actual transfer state machine's IO suspension points, including
//! journal custody on cancellation and the failed-part abort path.
mod store_support;
use object_store::path::Path;
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use store_support::{Harness, Reply, Script, run};
use unityrs_batch::{
    phase::Account,
    spool::journal::Journal,
    store::{
        client::{self, Settings},
        connector::Connector,
        multipart::{PartBudget, Target, Transfer},
        put::{PART_SIZE, Payload, Request},
    },
};
use unityrs_testkit::scratch::Scratch;
fn harness(replies: Vec<Reply>) -> Harness {
    let mut h = Harness::new([]);
    let mut script = Script::new(replies);
    script.yield_io = true;
    let url = store_support::endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let connector = Connector::new(
        script.clone(),
        client::origin(&settings).unwrap(),
        Duration::from_secs(2),
    );
    h.store = client::build(
        &settings,
        None,
        &store_support::credentials("AKIDEXAMPLE", "secret", None),
        &connector,
    )
    .unwrap();
    h.script = script;
    h
}
async fn cut<F: Future>(at: usize, future: F) -> Option<F::Output> {
    let mut future = Box::pin(future);
    let mut polls = 0;
    poll_fn(|cx| {
        if polls == at {
            return Poll::Ready(None);
        }
        polls += 1;
        future.as_mut().poll(cx).map(Some)
    })
    .await
}
fn create() -> Reply {
    Reply::status(200,"",b"<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><UploadId>u</UploadId></InitiateMultipartUploadResult>")
}
fn part() -> Reply {
    Reply::status(200, "ETag: \"part\"\r\n", b"")
}
fn complete() -> Reply {
    Reply::status(200,"",b"<CompleteMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><ETag>\"expected-2\"</ETag></CompleteMultipartUploadResult>")
}
#[test]
fn create_cancellation_leaves_no_partial_journal_record_and_releases_admission() {
    let mut finished = false;
    for at in 0..128 {
        let scratch = Scratch::new("transfer-create-drop");
        let journal = Journal::new(scratch.path()).unwrap();
        let h = harness(vec![create()]);
        let account = Account::new(32 << 20);
        let parts = PartBudget::new();
        let key = Path::parse("k").unwrap();
        let acknowledged = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let result = cut(
                at,
                Transfer::start(
                    &req,
                    &account,
                    &parts,
                    &journal,
                    Target {
                        key: &key,
                        endpoint: None,
                        bucket: "bucket",
                    },
                    PART_SIZE as u64 + 1,
                ),
            )
            .await;
            if let Some(result) = result {
                drop(result.unwrap());
                true
            } else {
                false
            }
        });
        assert_eq!(
            journal.reader().unwrap().next().unwrap().is_some(),
            acknowledged
        );
        assert_eq!(parts.used(), 0);
        assert_eq!(account.snapshot().input.used, 0);
        drop(h.slot.take_permit());
        assert_eq!(h.pool.snapshot().occupied, 0);
        if acknowledged {
            finished = true;
            break;
        }
    }
    assert!(finished);
}
#[test]
#[allow(clippy::too_many_lines)] // One cancellation matrix shares the identical transfer setup.
fn actual_part_complete_and_abort_drop_at_every_io_yield_keep_durable_custody() {
    for operation in ["part", "failed-part", "complete", "recovery", "abort"] {
        let mut finished = false;
        for at in 0..128 {
            let scratch = Scratch::new("transfer-drop");
            let journal = Journal::new(scratch.path()).unwrap();
            let mut replies = vec![create()];
            if operation != "abort" {
                replies.push(part());
            }
            if matches!(operation, "complete" | "recovery") {
                replies.push(part());
            }
            replies.extend(match operation {
                "part" => vec![part()],
                "failed-part" => vec![
                    Reply::error(403, "AccessDenied"),
                    Reply::status(204, "", b""),
                ],
                "complete" => vec![complete()],
                "recovery" => vec![
                    Reply::error(503, "SlowDown"),
                    Reply::error(404, "NoSuchUpload"),
                    Reply::status(
                        200,
                        "ETag: \"expected-2\"\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                        b"",
                    ),
                ],
                "abort" => vec![Reply::status(204, "", b"")],
                _ => unreachable!(),
            });
            let mut h = harness(replies);
            h.policy.attempts = 2;
            let account = Account::new(32 << 20);
            let parts = PartBudget::new();
            let key = Path::parse("k").unwrap();
            let acknowledged = run(async {
                let tries = h.tries();
                let req = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                );
                let mut transfer = Transfer::start(
                    &req,
                    &account,
                    &parts,
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
                if operation != "abort" {
                    transfer
                        .part(
                            &req,
                            Payload::read(&account, PART_SIZE, &mut std::io::repeat(b'a')).unwrap(),
                        )
                        .await
                        .unwrap();
                }
                if matches!(operation, "complete" | "recovery") {
                    transfer
                        .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                        .await
                        .unwrap();
                }
                let result = cut(at, async {
                    match operation {
                        "part" | "failed-part" => {
                            transfer
                                .part(&req, Payload::read(&account, 1, &mut &b"z"[..]).unwrap())
                                .await
                        }
                        "complete" | "recovery" => {
                            transfer.complete(&req, b"expected-2").await.map(drop)
                        }
                        "abort" => transfer.abort(&req).await,
                        _ => unreachable!(),
                    }
                })
                .await;
                if let Some(result) = result {
                    assert_eq!(result.is_ok(), operation != "failed-part");
                    true
                } else {
                    false
                }
            });
            assert_eq!(
                journal.reader().unwrap().next().unwrap().is_some(),
                !acknowledged || operation == "part",
                "{operation} cut {at}"
            );
            assert_eq!(account.snapshot().input.used, 0);
            assert_eq!(parts.used(), 0);
            drop(h.slot.take_permit());
            assert_eq!(h.pool.snapshot().occupied, 0);
            if acknowledged {
                finished = true;
                break;
            }
        }
        assert!(finished, "sweep must complete {operation}");
    }
}
