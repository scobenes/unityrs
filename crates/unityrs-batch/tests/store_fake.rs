//! Real loopback HTTP, independently verified signatures, and fake S3 state.
#[path = "store_support/fake_faults.rs"]
mod fake_faults;
mod store_support;
use object_store::{
    ObjectStore, ObjectStoreExt, PutPayload, multipart::MultipartStore, path::Path,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64},
};
use store_support::{
    fake::{Fake, Fault, Operation},
    sign,
};
use unityrs_batch::{
    http::{self, permit::PermitPool},
    store::{
        Kind,
        attempt::{Tries, TrySlot},
        classify::Ended,
        client::{self, Settings},
        connector::Connector,
    },
};

#[test]
fn independent_signature_verifier_matches_python_stdlib_reference() {
    let output = unityrs_testkit::child::output(
        std::process::Command::new("python3")
            .arg("-c")
            .arg(include_str!("store_support/sigv4.py")),
    )
    .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(sign::hex(&sign::hmac(&[0x0b; 20], b"Hi There")), lines[0]);
    assert_eq!(
        lines[0],
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    let mut request = hyper::Request::builder().method("PUT")
        .uri("/bucket/a%25%23%20%E9%9B%AA?list-type=2&prefix=a%20b&max-keys=1")
        .header("host","example.test:9000")
        .header("x-amz-date","20260101T123456Z")
        .header("x-amz-content-sha256",lines[2])
        .header("x-amz-security-token","session-sentinel")
        .header("authorization",format!("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token, Signature={}",lines[1]))
        .body(()).unwrap();
    let credentials = sign::Credentials {
        token: Some("session-sentinel".into()),
        skew: std::time::Duration::MAX,
        ..sign::Credentials::default()
    };
    assert!(sign::verify(
        request.method(),
        request.uri(),
        request.headers(),
        b"hello",
        &credentials
    ));
    assert!(!sign::verify(
        request.method(),
        request.uri(),
        request.headers(),
        b"tampered",
        &credentials
    ));
    request
        .headers_mut()
        .insert("x-amz-security-token", "wrong".parse().unwrap());
    assert!(!sign::verify(
        request.method(),
        request.uri(),
        request.headers(),
        b"hello",
        &credentials
    ));
}

#[test]
fn signed_put_get_head_and_listing_round_trip_through_loopback() {
    store_support::run(async {
        let fake = Fake::start().await;
        let (store, pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::parse("dir/a%#~ 雪").unwrap();
        let payload = PutPayload::from_static(b"payload");
        let result = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, payload.clone())
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(result.value.is_some());
        let got = tries
            .run(&slot, Kind::HeadRead, |_| async {
                store
                    .get(&key)
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(got.as_ref(), b"payload");
        let head = tries
            .run(&slot, Kind::Head, |_| async {
                store.head(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(head.size, 7);
        let list = tries
            .run(&slot, Kind::ListPage, |_| async {
                store
                    .list_with_delimiter(Some(&Path::from("dir")))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(list.objects.len(), 1);
        assert_eq!(list.objects[0].location, key);
        assert_eq!(
            fake.object("bucket", key.as_ref()).unwrap().bytes,
            b"payload"
        );
        let logs = fake.logs();
        assert_eq!(logs.len(), 4);
        assert!(logs.iter().all(|log| log.signature_valid));
        assert!(logs[0].raw_path.contains("%25%23~%20"));
        assert_eq!(logs[0].header("authorization"), Some("<redacted>"));
        assert_eq!(logs[0].body_len, 7);
        assert_ne!(logs[0].connection, logs[1].connection);
        drop(pool);
    });
}

#[test]
fn multipart_parts_complete_and_abort_have_s3_state_and_etags() {
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
        let key = Path::from("archive.zip");
        let id = tries
            .run(&slot, Kind::Create, |_| async {
                store.create_multipart(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(fake.open_uploads(), 1);
        let first = tries
            .run(&slot, Kind::Part, |_| async {
                store
                    .put_part(&key, &id, 0, PutPayload::from_static(b"hello"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        let second = tries
            .run(&slot, Kind::Part, |_| async {
                store
                    .put_part(&key, &id, 1, PutPayload::from_static(b"world"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        let parts = vec![first, second];
        let result = tries
            .run(&slot, Kind::Complete, |_| async {
                store
                    .complete_multipart(&key, &id, parts.clone())
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(
            result.e_tag.as_deref(),
            Some("\"065947336a2f2a95ba8899f3675c3be6-2\"")
        );
        assert_eq!(
            fake.object("bucket", "archive.zip").unwrap().bytes,
            b"helloworld"
        );
        assert_eq!(fake.open_uploads(), 0);
        let id = tries
            .run(&slot, Kind::Create, |_| async {
                store.create_multipart(&key).await.map_err(|_| Ended::Error)
            })
            .await
            .unwrap()
            .value
            .unwrap();
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
fn scheduled_service_error_is_retried_and_logged_per_try() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(Operation::Put, "key", 1, Fault::error(503, "SlowDown"));
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(2),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("key");
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
        assert_eq!(
            logs.iter().map(|log| log.try_ordinal).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(unsettled.load(std::sync::atomic::Ordering::Acquire), 1);
    });
}

#[test]
fn listing_paginates_in_utf8_order_and_preserves_folder_markers() {
    use object_store::list::{PaginatedListOptions, PaginatedListStore};
    store_support::run(async {
        let fake = Fake::start().await;
        for key in [
            "prefix/雪",
            "prefix/~",
            "prefix/a%",
            "prefix/a#",
            "prefix/folder/",
        ] {
            fake.seed("bucket", key, b"");
        }
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let mut token = None;
        let mut keys = Vec::new();
        loop {
            let options = PaginatedListOptions {
                max_keys: Some(2),
                page_token: token.clone(),
                ..Default::default()
            };
            let page = tries
                .run(&slot, Kind::ListPage, |_| async {
                    store
                        .list_paginated(Some("prefix/"), options.clone())
                        .await
                        .map_err(|_| Ended::Error)
                })
                .await
                .unwrap()
                .value
                .unwrap();
            keys.extend(
                page.result
                    .objects
                    .into_iter()
                    .map(|object| object.location.to_string()),
            );
            token = page.page_token;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            keys,
            [
                "prefix/a#",
                "prefix/a%",
                "prefix/folder",
                "prefix/~",
                "prefix/雪"
            ]
        );
        assert!(fake.object("bucket", "prefix/folder/").is_some());
        assert_eq!(fake.logs().len(), 3);
        assert!(
            fake.logs()[1]
                .raw_query
                .as_deref()
                .unwrap()
                .contains("continuation-token=offset%3A2")
        );
    });
}

#[test]
fn conditional_put_checks_create_and_exact_update_etag() {
    use object_store::{PutMode, PutOptions, UpdateVersion};
    store_support::run(async {
        let fake = Fake::start().await;
        fake.seed("bucket", "head", b"old");
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("head");
        for mode in [
            PutMode::Create,
            PutMode::Update(UpdateVersion {
                e_tag: Some("\"wrong\"".into()),
                version: None,
            }),
        ] {
            let done = tries
                .run(
                    &slot,
                    Kind::HeadWrite {
                        if_match: matches!(mode, PutMode::Update(_)),
                    },
                    |_| async {
                        store
                            .put_opts(
                                &key,
                                PutPayload::from_static(b"new"),
                                PutOptions {
                                    mode: mode.clone(),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(|_| Ended::Error)
                    },
                )
                .await
                .unwrap();
            assert_eq!(done.record.status, Some(412));
            assert_eq!(fake.object("bucket", "head").unwrap().bytes, b"old");
        }
        let tag = fake.object("bucket", "head").unwrap().etag;
        let done = tries
            .run(&slot, Kind::HeadWrite { if_match: true }, |_| async {
                store
                    .put_opts(
                        &key,
                        PutPayload::from_static(b"new"),
                        PutOptions {
                            mode: PutMode::Update(UpdateVersion {
                                e_tag: Some(tag.clone()),
                                version: None,
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert_eq!(fake.object("bucket", "head").unwrap().bytes, b"new");
    });
}

#[test]
fn late_commit_is_visible_after_an_answered_put() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "late",
            1,
            Fault {
                commit_delay: Some(std::time::Duration::from_millis(30)),
                ..Fault::default()
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
        let key = Path::from("late");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"late bytes"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert!(fake.object("bucket", "late").is_none());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(fake.object("bucket", "late").unwrap().bytes, b"late bytes");
    });
}

#[test]
fn a_redirect_is_logged_and_never_followed() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "redirect",
            1,
            Fault::redirect(307, "http://unreachable.invalid/stolen"),
        );
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("redirect");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"x"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert_eq!(done.record.status, Some(307));
        assert_eq!(fake.logs().len(), 1);
        assert!(fake.object("bucket", "redirect").is_none());
    });
}

#[test]
fn dropped_gets_at_each_response_stage_release_the_permit() {
    store_support::run(async {
        for fault in [
            Fault {
                before_head: Some(std::time::Duration::from_secs(30)),
                ..Default::default()
            },
            Fault {
                after_head: Some(std::time::Duration::from_secs(30)),
                ..Default::default()
            },
            Fault {
                mid_body: Some((8, std::time::Duration::from_secs(30))),
                ..Default::default()
            },
        ] {
            let fake = Fake::start().await;
            fake.seed("bucket", "held", &[1; 256]);
            fake.schedule(Operation::Get, "held", 1, fault);
            let (store, pool, slot) = store(&fake);
            let stop = AtomicBool::new(false);
            let unsettled = AtomicU64::new(0);
            let tries = Tries {
                policy: store_support::policy(1),
                control: &stop,
                unsettled: &unsettled,
            };
            let key = Path::from("held");
            let future = tries.run(&slot, Kind::HeadRead, |_| async {
                store
                    .get(&key)
                    .await
                    .map_err(|_| Ended::Error)?
                    .bytes()
                    .await
                    .map_err(|_| Ended::Error)
            });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), future)
                    .await
                    .is_err()
            );
            assert_eq!(fake.logs().len(), 1);
            drop(slot);
            assert!(pool.acquire().is_some());
        }
    });
}

#[test]
fn stopping_mid_put_holds_no_committed_object() {
    store_support::run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Put,
            "held",
            1,
            Fault {
                stop_reading_after: Some(1024),
                ..Default::default()
            },
        );
        let (store, pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("held");
        let payload = PutPayload::from(vec![0; 256 * 1024]);
        let future = tries.run(&slot, Kind::Put, |_| async {
            store
                .put(&key, payload.clone())
                .await
                .map_err(|_| Ended::Error)
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), future)
                .await
                .is_err()
        );
        assert_eq!(fake.logs()[0].body_len, 1024);
        assert!(fake.object("bucket", "held").is_none());
        drop(slot);
        assert!(pool.acquire().is_some());
    });
}

#[cfg(feature = "test-hooks")]
#[test]
fn https_uses_the_existing_test_root_and_checks_signatures() {
    store_support::run(async {
        let fake = Fake::start_with(store_support::fake::Config {
            tls: true,
            ..Default::default()
        })
        .await;
        let (store, _pool, slot) = store(&fake);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("tls");
        let done = tries
            .run(&slot, Kind::Put, |_| async {
                store
                    .put(&key, PutPayload::from_static(b"encrypted"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await
            .unwrap();
        assert!(done.value.is_some());
        assert!(fake.logs()[0].signature_valid);
        assert_eq!(fake.object("bucket", "tls").unwrap().bytes, b"encrypted");
    });
}

#[cfg(feature = "test-hooks")]
#[test]
fn virtual_hosted_bucket_uses_only_the_resolver_hook() {
    store_support::run(async {
        let mut fake = Fake::start_with(store_support::fake::Config {
            path_style: false,
            ..Default::default()
        })
        .await;
        fake.endpoint.set_host(Some("store.test")).unwrap();
        let (store, _pool, slot) = store_mode(&fake, false);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let key = Path::from("hosted");
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
        assert_eq!(fake.logs()[0].raw_path, "/hosted");
        assert!(
            fake.logs()[0]
                .header("host")
                .unwrap()
                .starts_with("bucket.store.test:")
        );
    });
}

fn store(fake: &Fake) -> (client::Store, PermitPool, Arc<TrySlot>) {
    store_mode(fake, true)
}
fn store_mode(fake: &Fake, path_style: bool) -> (client::Store, PermitPool, Arc<TrySlot>) {
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&fake.endpoint),
        path_style,
    };
    let table = Arc::new(unityrs_batch::sys::reset::SocketTable::new());
    let transport = http::Service::new(tokio::runtime::Handle::current(), table).unwrap();
    #[cfg(feature = "test-hooks")]
    let transport = transport
        .with_test_hooks(
            Arc::new(store_support::fake::Resolver::new(fake)),
            &[rustls::pki_types::CertificateDer::from(
                store_support::fake::ROOT.to_vec(),
            )],
        )
        .unwrap();
    let connector = Connector::new(
        transport,
        client::origin(&settings).unwrap(),
        store_support::SECOND,
    );
    let credentials = store_support::credentials("AKIDEXAMPLE", "secret", None);
    let store = client::build(&settings, None, &credentials, &connector).unwrap();
    let pool = PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
    (store, pool, slot)
}
