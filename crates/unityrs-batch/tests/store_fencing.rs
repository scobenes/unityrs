mod store_support;
use hyper::body::Bytes;
use object_store::path::Path;
use std::{
    future::{Future, poll_fn},
    sync::atomic::Ordering,
    task::Poll,
    time::Duration,
};
use store_support::{Harness, Reply, Script, run};
use unityrs_batch::store::{
    client::{self, Settings},
    connector::Connector,
    head,
    put::Request,
};
fn identity() -> Vec<u8> {
    format!(r#"{{"manifest_version":1,"run_id":"run","attempt":1,"write_seq":0,"binary_version":"test","spec_version":1,"spec_sha256":"{}"}}"#,"0".repeat(64)).into_bytes()
}
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
#[test]
fn running_history_retains_unsettled_and_retryable_4xx_across_later_answers() {
    for (first, needs_final) in [
        (Reply::error(503, "SlowDown"), true),
        (Reply::error(408, "RequestTimeout"), true),
        (Reply::error(429, "SlowDown"), true),
        (Reply::Refuse(unityrs_batch::http::Detail::Connect), false),
    ] {
        let mut h = harness(vec![first, Reply::error(403, "AccessDenied")]);
        h.policy.attempts = 2;
        let fence = head::Fence::default();
        let key = Path::parse("head").unwrap();
        let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(2),
            );
            let _ = head::Write {
                key: &key,
                run_id: "run",
                identity: unityrs_batch::manifest::head::Identity {
                    attempt: 2,
                    write_seq: 0,
                },
                bytes: Bytes::from_static(b"head"),
                conditional: true,
            }
            .publish(&req, &fence, &mut buffer)
            .await;
        });
        assert_eq!(fence.history().needs_final_after_stop(), needs_final);
        assert!(!fence.history().succeeded);
    }
}
#[test]
fn stop_after_readback_does_not_replace_the_last_attempted_precondition() {
    let h = harness(vec![
        Reply::error(412, "PreconditionFailed"),
        Reply::status(
            200,
            "ETag: \"prior\"\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
            &identity(),
        ),
    ]);
    let fence = head::Fence::default();
    let key = Path::parse("head").unwrap();
    let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        let write = head::Write {
            key: &key,
            run_id: "run",
            identity: unityrs_batch::manifest::head::Identity {
                attempt: 2,
                write_seq: 0,
            },
            bytes: Bytes::from_static(b"head"),
            conditional: true,
        };
        let mut future = Box::pin(write.publish(&req, &fence, &mut buffer));
        poll_fn(|cx| {
            if h.script.seen.count() == 2 {
                h.stop.store(true, Ordering::Release);
            }
            match future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    assert!(result.is_err());
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
    });
    assert_eq!(fence.last(), None);
    assert!(!fence.history().needs_final_after_stop());
    assert_eq!(h.script.seen.count(), 2);
}

struct FailReadback {
    script: Script,
    calls: std::sync::atomic::AtomicUsize,
    memory: bool,
}
impl unityrs_batch::store::connector::Transport for FailReadback {
    type Io = store_support::ScriptIo;
    async fn send(
        &self,
        permit: &mut unityrs_batch::http::permit::RequestPermit,
        url: &url::Url,
        request: hyper::Request<unityrs_batch::http::Payload>,
        timeout: Duration,
        token: unityrs_batch::http::permit::OperationToken,
        connected: &std::sync::atomic::AtomicBool,
    ) -> Result<unityrs_batch::http::Response<Self::Io>, unityrs_batch::http::Error> {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 1 {
            drop(token);
            return Err(if self.memory {
                unityrs_batch::http::Error::Memory
            } else {
                unityrs_batch::http::Error::InvalidRequest
            });
        }
        self.script
            .send(permit, url, request, timeout, token, connected)
            .await
    }
}
#[test]
fn readback_engine_failures_are_never_downgraded_to_unreachable() {
    for memory in [false, true] {
        let url = store_support::endpoint();
        let settings = Settings {
            bucket: "bucket",
            region: "us-east-1",
            endpoint: Some(&url),
            path_style: true,
        };
        let connector = Connector::new(
            FailReadback {
                script: Script::new([Reply::error(412, "PreconditionFailed")]),
                calls: std::sync::atomic::AtomicUsize::new(0),
                memory,
            },
            client::origin(&settings).unwrap(),
            Duration::from_secs(1),
        );
        let store = client::build(
            &settings,
            None,
            &store_support::credentials("AKIDEXAMPLE", "secret", None),
            &connector,
        )
        .unwrap();
        let h = Harness::new([]);
        let fence = head::Fence::default();
        let key = Path::parse("head").unwrap();
        let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
        let result = run(async {
            let tries = h.tries();
            let req = Request::new(
                &store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(2),
            );
            head::Write {
                key: &key,
                run_id: "run",
                identity: unityrs_batch::manifest::head::Identity {
                    attempt: 2,
                    write_seq: 0,
                },
                bytes: Bytes::from_static(b"head"),
                conditional: true,
            }
            .publish(&req, &fence, &mut buffer)
            .await
        });
        assert_eq!(
            result.unwrap_err(),
            head::Error::Operation(unityrs_batch::store::put::Error::Fault(if memory {
                unityrs_batch::store::attempt::Fault::Memory
            } else {
                unityrs_batch::store::attempt::Fault::Internal
            }))
        );
    }
}

#[test]
fn actual_head_write_drop_at_every_io_yield_retains_earlier_unsettled_facts() {
    let mut finished = false;
    for cut in 0..128 {
        let mut h = harness(vec![
            Reply::error(503, "SlowDown"),
            Reply::error(412, "PreconditionFailed"),
            Reply::status(
                200,
                "ETag: \"prior\"\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                &identity(),
            ),
            Reply::status(200, "ETag: \"written\"\r\n", b""),
        ]);
        h.policy.attempts = 2;
        let fence = head::Fence::default();
        let key = Path::parse("head").unwrap();
        let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
        let done = run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(2),
            );
            let write = head::Write {
                key: &key,
                run_id: "run",
                identity: unityrs_batch::manifest::head::Identity {
                    attempt: 2,
                    write_seq: 0,
                },
                bytes: Bytes::from_static(b"head"),
                conditional: true,
            };
            let mut future = Box::pin(write.publish(&req, &fence, &mut buffer));
            let mut polls = 0;
            poll_fn(|cx| {
                if polls == cut {
                    return Poll::Ready(false);
                }
                polls += 1;
                match future.as_mut().poll(cx) {
                    Poll::Ready(result) => {
                        result.unwrap();
                        Poll::Ready(true)
                    }
                    Poll::Pending => Poll::Pending,
                }
            })
            .await
        });
        if h.script.seen.count() >= 2 {
            assert!(fence.history().unsettled);
        }
        drop(h.slot.take_permit());
        assert_eq!(h.pool.snapshot().occupied, 0);
        if done {
            assert!(fence.history().succeeded);
            assert!(!fence.history().needs_final_after_stop());
            finished = true;
            break;
        }
        if cut > 0 {
            assert!(fence.history().needs_final_after_stop());
        }
    }
    assert!(finished);
}

#[test]
fn handover_can_snapshot_history_while_the_write_future_is_frozen() {
    let h = harness(vec![Reply::StallConnect]);
    let fence = head::Fence::default();
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        let key = Path::parse("head").unwrap();
        let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
        let write = head::Write {
            key: &key,
            run_id: "run",
            identity: unityrs_batch::manifest::head::Identity {
                attempt: 2,
                write_seq: 0,
            },
            bytes: Bytes::from_static(b"head"),
            conditional: true,
        };
        let mut future = Box::pin(write.publish(&req, &fence, &mut buffer));
        poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(fence.history().needs_final_after_stop());
        assert_eq!(fence.last(), None);
        drop(future);
    });
}

#[test]
fn an_engine_failure_before_connect_settles_head_history_without_a_false_inflight_try() {
    let url = store_support::endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let connector = Connector::new(
        FailReadback {
            script: Script::new([]),
            calls: std::sync::atomic::AtomicUsize::new(1),
            memory: true,
        },
        client::origin(&settings).unwrap(),
        Duration::from_secs(1),
    );
    let store = client::build(
        &settings,
        None,
        &store_support::credentials("AKIDEXAMPLE", "secret", None),
        &connector,
    )
    .unwrap();
    let h = Harness::new([]);
    let fence = head::Fence::default();
    let key = Path::parse("head").unwrap();
    let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        assert_eq!(
            head::Write {
                key: &key,
                run_id: "run",
                identity: unityrs_batch::manifest::head::Identity {
                    attempt: 2,
                    write_seq: 0
                },
                bytes: Bytes::from_static(b"head"),
                conditional: true
            }
            .publish(&req, &fence, &mut buffer)
            .await
            .unwrap_err(),
            head::Error::Operation(unityrs_batch::store::put::Error::Fault(
                unityrs_batch::store::attempt::Fault::Memory
            ))
        );
    });
    assert!(fence.history().attempted);
    assert!(!fence.history().needs_final_after_stop());
    assert!(!fence.history().in_flight);
}
