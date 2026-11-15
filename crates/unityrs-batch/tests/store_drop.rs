//! Cancellation before every observed IO yield: permit custody, charged payloads
//! and unsettled accounting. Existing `store_tries` covers timer/lookup quarantine.
mod store_support;
use hyper::body::Bytes;
use object_store::{
    PutPayload,
    multipart::{MultipartStore, PartId},
    path::Path,
};
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use store_support::{Harness, Reply, Script, run};
use unityrs_batch::{
    phase::Account,
    store::{
        Kind,
        attempt::Tries,
        classify::Ended,
        client::{self, Settings},
        connector::Connector,
        head,
        put::{Payload, Request},
    },
};
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
fn ok() -> Reply {
    Reply::status(200, "ETag: \"tag\"\r\n", b"")
}
fn body() -> Vec<u8> {
    format!(r#"{{"manifest_version":1,"run_id":"run","attempt":1,"write_seq":0,"binary_version":"test","spec_version":1,"spec_sha256":"{}"}}"#,"0".repeat(64)).into_bytes()
}
#[test]
fn dropping_put_and_head_read_at_every_io_yield_releases_their_buffers_and_permit() {
    for head_read in [false, true] {
        let mut completed = false;
        for cut in 0..128 {
            let h = harness(vec![if head_read {
                Reply::status(
                    200,
                    "ETag: \"h\"\r\nLast-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n",
                    &body(),
                )
            } else {
                ok()
            }]);
            let account = Account::new(2 << 20);
            let path = Path::parse("k").unwrap();
            let done = run(async {
                let tries = h.tries();
                let request = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(2),
                );
                let payload = Payload::read(&account, 3, &mut &b"png"[..]).unwrap();
                let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
                let mut future = Box::pin(async {
                    if head_read {
                        drop(payload);
                        head::read(&request, &path, "run", &mut buffer)
                            .await
                            .map(drop)
                            .map_err(|_| ())
                    } else {
                        request
                            .put(&path, payload, "image/png", b"tag")
                            .await
                            .map(drop)
                            .map_err(|_| ())
                    }
                });
                let mut polls = 0;
                let done = poll_fn(|cx| {
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
                .await;
                drop(future);
                done
            });
            assert_eq!(account.snapshot().input.used, 0, "cut {cut}");
            drop(h.slot.take_permit());
            assert_eq!(h.pool.snapshot().occupied, 0, "cut {cut}");
            if done {
                completed = true;
                break;
            }
            if cut > 0 && !head_read {
                assert_eq!(h.unsettled(), 1);
            }
        }
        assert!(completed, "sweep must reach a successful operation");
    }
}
#[test]
fn dropping_create_part_complete_abort_at_every_io_yield_never_leaks_custody() {
    for kind in [Kind::Create, Kind::Part, Kind::Complete, Kind::Abort] {
        let mut completed = false;
        for cut in 0..128 {
            let response=match kind {
                Kind::Create=>Reply::status(200,"",b"<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><UploadId>u</UploadId></InitiateMultipartUploadResult>"),
                Kind::Complete=>Reply::status(200,"",b"<CompleteMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><ETag>\"tag\"</ETag></CompleteMultipartUploadResult>"),
                _=>ok(),
            };
            let h = harness(vec![response]);
            let path = Path::parse("k").unwrap();
            let id = "upload".to_owned();
            let done = run(async {
                let tries: Tries<'_> = h.tries();
                let request = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(2),
                );
                let mut future = Box::pin(request.run(kind, |_| async {
                    match kind {
                        Kind::Create => h.store.create_multipart(&path).await.map(drop),
                        Kind::Part => h
                            .store
                            .put_part(&path, &id, 0, PutPayload::from(Bytes::from_static(b"part")))
                            .await
                            .map(drop),
                        Kind::Complete => h
                            .store
                            .complete_multipart(
                                &path,
                                &id,
                                vec![PartId {
                                    content_id: "\"part\"".into(),
                                }],
                            )
                            .await
                            .map(drop),
                        Kind::Abort => h.store.abort_multipart(&path, &id).await,
                        _ => unreachable!(),
                    }
                    .map_err(|_| Ended::Error)
                }));
                let mut polls = 0;
                let done = poll_fn(|cx| {
                    if polls == cut {
                        return Poll::Ready(false);
                    }
                    polls += 1;
                    match future.as_mut().poll(cx) {
                        Poll::Ready(result) => {
                            assert!(result.unwrap().value.is_some());
                            Poll::Ready(true)
                        }
                        Poll::Pending => Poll::Pending,
                    }
                })
                .await;
                drop(future);
                done
            });
            drop(h.slot.take_permit());
            assert_eq!(h.pool.snapshot().occupied, 0, "{kind:?} cut {cut}");
            if done {
                completed = true;
                break;
            }
            if cut > 0 {
                assert_eq!(h.unsettled(), u64::from(kind.counted()));
            }
        }
        assert!(completed);
    }
}
