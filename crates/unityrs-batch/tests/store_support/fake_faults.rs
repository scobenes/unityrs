//! Fault controls exercised against actual sockets and engine try accounting.
use super::{store, store_support};
use object_store::{ObjectStoreExt, PutPayload, multipart::MultipartStore, path::Path};
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use store_support::fake::{Fake, Fault, Operation};
use unityrs_batch::store::{Kind, attempt::Tries, classify::Ended};

#[test]
fn each_completed_request_closes_with_a_peer_observed_reset() {
    store_support::run(async {
        let fake = Fake::start().await;
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("reset");
        for _ in 0..2 {
            let done = tries
                .run(&slot, Kind::Put, |_| async {
                    store
                        .put(&key, PutPayload::from_static(b"x"))
                        .await
                        .map_err(|_| Ended::Error)
                })
                .await
                .unwrap();
            assert!(done.value.is_some());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        let logs = fake.logs();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().all(|log| log.reset));
        assert_ne!(logs[0].connection, logs[1].connection);
    });
}

#[test]
fn retry_after_is_capped_and_measured_at_the_next_accept() {
    store_support::run(async {
        let fake = Fake::start().await;
        let mut fault = Fault::error(503, "SlowDown");
        fault.headers.push(("retry-after".into(), "1".into()));
        fake.schedule(Operation::Put, "retry", 1, fault);
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let mut policy = store_support::policy(2);
        policy.backoff_max_ms = 20;
        let tries = Tries {
            policy,
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("retry");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"x"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        let logs = fake.logs();
        assert_eq!(logs.len(), 2);
        let elapsed = logs[1].accepted.duration_since(logs[0].accepted);
        assert!(elapsed >= Duration::from_millis(20), "{elapsed:?}");
        assert!(
            elapsed < Duration::from_secs(1),
            "Retry-After must be capped: {elapsed:?}"
        );
    });
}

#[test]
fn a_timed_out_try_replaced_by_an_answered_one_stays_unsettled() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "timed",
            1,
            Fault {
                before_head: Some(Duration::from_millis(100)),
                ..Default::default()
            },
        );
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let mut policy = store_support::policy(2);
        policy.timeout = Duration::from_millis(20);
        let tries = Tries {
            policy,
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("timed");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"x"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert_eq!(done.tries, 2);
        assert_eq!(fake.logs().len(), 2);
        assert_eq!(unsettled.load(Ordering::Acquire), 1);
    });
}

#[test]
fn lost_complete_response_commits_once_then_returns_no_such_upload() {
    store_support::run(async {
        let fake = Fake::start().await;
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(2),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("lost");
        let id = tries
            .run(&slot, Kind::Create, |_| async {
                store.create_multipart(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        let part = tries
            .run(&slot, Kind::Part, |_| async {
                store
                    .put_part(&key, &id, 0, PutPayload::from_static(b"complete"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        fake.schedule(
            Operation::Complete,
            "lost",
            1,
            Fault {
                lost_response: true,
                ..Default::default()
            },
        );
        let done = tries
            .run(&slot, Kind::Complete, |_| async {
                store
                    .complete_multipart(&key, &id, vec![part.clone()])
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert_eq!(done.record.status, Some(404));
        assert!(done.record.code.is("NoSuchUpload"));
        assert_eq!(done.tries, 2);
        assert_eq!(unsettled.load(Ordering::Acquire), 1);
        assert_eq!(fake.object("bucket", "lost").unwrap().bytes, b"complete");
        assert_eq!(fake.open_uploads(), 0);
    });
}

#[test]
fn a_200_complete_error_leaves_the_upload_open() {
    store_support::run(async {
        let fake = Fake::start().await;
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("error");
        let id = tries
            .run(&slot, Kind::Create, |_| async {
                store.create_multipart(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        let part = tries
            .run(&slot, Kind::Part, |_| async {
                store
                    .put_part(&key, &id, 0, PutPayload::from_static(b"x"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        fake.schedule(
            Operation::Complete,
            "error",
            1,
            Fault::error(200, "InternalError"),
        );
        let done = tries
            .run(&slot, Kind::Complete, |_| async {
                store
                    .complete_multipart(&key, &id, vec![part.clone()])
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_none());
        assert_eq!(done.record.status, Some(200));
        assert!(done.record.code.is("InternalError"));
        assert_eq!(fake.open_uploads(), 1);
        assert!(fake.object("bucket", "error").is_none());
        assert_eq!(unsettled.load(Ordering::Acquire), 1);
    });
}

#[test]
fn an_oversized_upload_id_can_be_returned_and_aborted_by_exact_id() {
    store_support::run(async {
        let fake = Fake::start().await;
        let long = "i".repeat(1025);
        fake.schedule(
            Operation::Create,
            "oversize",
            1,
            Fault {
                upload_id: Some(long.clone()),
                ..Default::default()
            },
        );
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("oversize");
        let id = tries
            .run(&slot, Kind::Create, |_| async {
                store.create_multipart(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(id, long);
        assert_eq!(fake.open_uploads(), 1);
        let done = tries
            .run(&slot, Kind::Abort, |_| async {
                store
                    .abort_multipart(&key, &id)
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert_eq!(fake.open_uploads(), 0);
    });
}

#[test]
fn non_md5_and_oversized_etags_are_returned_verbatim() {
    store_support::run(async {
        let fake = Fake::start().await;
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("tag");
        for (ordinal, tag) in [
            (1, "\"opaque-tag\"".into()),
            (2, format!("\"{}\"", "t".repeat(101))),
        ] {
            fake.schedule(
                Operation::Put,
                "tag",
                ordinal,
                Fault {
                    etag: Some(tag.clone()),
                    ..Default::default()
                },
            );
            let done = tries
                .run(&slot, Kind::Put, |_| async {
                    store
                        .put(&key, PutPayload::from_static(b"x"))
                        .await
                        .map_err(|_| Ended::Error)
                })
                .await
                .unwrap();
            assert_eq!(done.value.unwrap().e_tag.as_deref(), Some(tag.as_str()));
        }
    });
}

#[test]
fn a_close_after_the_request_has_no_commit_or_response_status() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "close",
            1,
            Fault {
                close: true,
                ..Default::default()
            },
        );
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("close");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"x"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_none());
        assert_eq!(done.record.status, None);
        assert_eq!(unsettled.load(Ordering::Acquire), 1);
        assert!(fake.object("bucket", "close").is_none());
    });
}

#[test]
fn slow_request_reads_delay_the_response_by_each_kibibyte() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "slow",
            1,
            Fault {
                read_delay_per_kib: Some(Duration::from_millis(5)),
                ..Default::default()
            },
        );
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("slow");
        let started = Instant::now();
        let payload = PutPayload::from(vec![0; 4096]);
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, payload.clone())
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert!(started.elapsed() >= Duration::from_millis(20));
        assert_eq!(fake.logs()[0].body_len, 4096);
    });
}
