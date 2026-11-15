//! The B11b library operations over the independently signed loopback peer.
mod store_support;
use hyper::body::Bytes;
use object_store::path::Path;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
    time::Duration,
};
use store_support::{
    fake::{Fake, Fault, Operation},
    run,
};
use unityrs_batch::{
    digest::Incremental,
    http::{self, permit::PermitPool},
    manifest::{Family, chunks::Destination as _, head::Identity},
    phase::{Account, Buffer},
    spool::journal::Journal,
    store::{
        Kind, abort,
        attempt::{Tries, TrySlot},
        client::{self, Settings},
        connector::Connector,
        head,
        multipart::{PartBudget, Target, Transfer},
        put::{PART_SIZE, Payload, Request},
        rows,
    },
};
use unityrs_testkit::scratch::Scratch;
fn store(fake: &Fake, content_type: Option<&str>) -> (client::Store, PermitPool, Arc<TrySlot>) {
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&fake.endpoint),
        path_style: true,
    };
    let service = http::Service::new(
        tokio::runtime::Handle::current(),
        Arc::new(unityrs_batch::sys::reset::SocketTable::default()),
    )
    .unwrap();
    let connector = Connector::new(
        service,
        client::origin(&settings).unwrap(),
        Duration::from_secs(2),
    );
    let store = client::build(
        &settings,
        content_type,
        &store_support::credentials("AKIDEXAMPLE", "secret", None),
        &connector,
    )
    .unwrap();
    let pool = PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
    (store, pool, slot)
}
fn identity(attempt: u32, seq: u64) -> Bytes {
    Bytes::from(format!(
        r#"{{"manifest_version":1,"run_id":"run","attempt":{attempt},"write_seq":{seq},"binary_version":"test","spec_version":1,"spec_sha256":"{}"}}"#,
        "0".repeat(64)
    ))
}
#[test]
fn transfer_recovers_a_lost_complete_with_head_and_no_orphan_upload() {
    let scratch = Scratch::new("library-multipart");
    let journal = Journal::new(scratch.path()).unwrap();
    run(async {
        let fake = Fake::start().await;
        fake.schedule(
            Operation::Complete,
            "archive.zip",
            1,
            Fault {
                lost_response: true,
                ..Default::default()
            },
        );
        let (store, _pool, slot) = store(&fake, Some("application/zip"));
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(2),
            control: &stop,
            unsettled: &unsettled,
        };
        let req = Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let account = Account::new(32 << 20);
        let parts = PartBudget::new();
        let key = Path::parse("archive.zip").unwrap();
        let mut transfer = Transfer::start(
            &req,
            &account,
            &parts,
            &journal,
            Target {
                key: &key,
                endpoint: Some(fake.endpoint.as_str()),
                bucket: "bucket",
            },
            PART_SIZE as u64 + 1,
        )
        .await
        .unwrap();
        let mut hashes = Incremental::default();
        for size in [PART_SIZE, 1] {
            let payload = Payload::read(&account, size, &mut std::io::repeat(b'a')).unwrap();
            hashes.update(payload.as_ref()).unwrap();
            transfer.part(&req, payload).await.unwrap();
        }
        assert!(
            !transfer
                .complete(&req, hashes.finish().etag.to_string().as_bytes())
                .await
                .unwrap()
                .verify_by_size
        );
        assert!(journal.reader().unwrap().next().unwrap().is_none());
        assert_eq!(fake.open_uploads(), 0);
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(parts.used(), 0);
        assert_eq!(unsettled.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(
            fake.object("bucket", "archive.zip").unwrap().bytes.len(),
            PART_SIZE + 1
        );
        let logs = fake.logs();
        assert!(logs.iter().all(|log| log.signature_valid));
        assert_eq!(
            logs.iter()
                .filter(|log| log.operation == Operation::Complete)
                .count(),
            2
        );
        assert_eq!(
            logs.iter()
                .filter(|log| log.operation == Operation::Head)
                .count(),
            1
        );
        assert_eq!(
            logs.iter()
                .find(|log| log.operation == Operation::Create)
                .unwrap()
                .header("content-type"),
            Some("application/zip")
        );
    });
}
#[test]
fn a_dropped_upload_is_recovered_by_the_matching_startup_abort_library() {
    let scratch = Scratch::new("library-abort");
    let journal = Journal::new(scratch.path()).unwrap();
    run(async {
        let fake = Fake::start().await;
        let (store, _pool, slot) = store(&fake, Some("image/png"));
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let req = Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(3),
        );
        let account = Account::new(32 << 20);
        let parts = PartBudget::new();
        let key = Path::parse("unfinished").unwrap();
        drop(
            Transfer::start(
                &req,
                &account,
                &parts,
                &journal,
                Target {
                    key: &key,
                    endpoint: Some(fake.endpoint.as_str()),
                    bucket: "bucket",
                },
                PART_SIZE as u64 + 1,
            )
            .await
            .unwrap(),
        );
        assert_eq!(fake.open_uploads(), 1);
        let report =
            abort::recover_journal(&req, &journal, Some(fake.endpoint.as_str()), "bucket").await;
        assert_eq!((report.aborted, report.failed), (1, 0));
        assert_eq!(fake.open_uploads(), 0);
        assert!(journal.reader().unwrap().next().unwrap().is_none());
    });
}
#[test]
fn fencing_reads_a_conflict_and_refuses_a_higher_attempt_on_the_real_peer() {
    run(async {
        let fake = Fake::start().await;
        let key = Path::parse("_unityrs/run/manifest.json").unwrap();
        fake.seed("bucket", key.as_ref(), &identity(1, 0));
        let (store, _pool, slot) = store(&fake, None);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let req = Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(3),
        );
        let mut buffer = vec![0; usize::try_from(unityrs_batch::store::HEAD_CAP).unwrap()];
        let before = head::preflight(&req, "", &key, "run", 2, &mut buffer)
            .await
            .unwrap()
            .unwrap();
        let fence = head::Fence::default();
        let running = head::Write {
            key: &key,
            run_id: "run",
            identity: Identity {
                attempt: 2,
                write_seq: 0,
            },
            bytes: identity(2, 0),
            conditional: true,
        }
        .publish(&req, &fence, &mut buffer)
        .await
        .unwrap();
        assert_eq!(fence.last(), Some(before.etag));
        fake.seed("bucket", key.as_ref(), &identity(3, 0));
        let fence = head::Fence::new(Some(running));
        assert_eq!(
            head::Write {
                key: &key,
                run_id: "run",
                identity: Identity {
                    attempt: 2,
                    write_seq: 1
                },
                bytes: identity(2, 1),
                conditional: true
            }
            .publish(&req, &fence, &mut buffer)
            .await
            .unwrap_err(),
            head::Error::Superseded
        );
        assert_eq!(
            fake.object("bucket", key.as_ref()).unwrap().bytes,
            identity(3, 0)
        );
    });
}
#[test]
fn rows_use_the_owned_chunk_seam_on_the_finisher_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fake = runtime.block_on(Fake::start());
    let (store, _pool, slot) = runtime.block_on(async { store(&fake, None) });
    let stop = AtomicBool::new(false);
    let unsettled = AtomicU64::new(0);
    let tries = Tries {
        policy: store_support::policy(1),
        control: &stop,
        unsettled: &unsettled,
    };
    let account = Account::new(24 << 20);
    let mut buffer = Buffer::new(account.meter(), 16 << 20).unwrap();
    buffer.extend(b"{}\n").unwrap();
    let mut dest = rows::Destination {
        runtime: &runtime,
        request: Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(3),
        ),
        account: &account,
        prefix: "",
        run_id: "run",
        attempt: 1,
    };
    assert!(
        dest.publish_owned(Family::Outputs, 0, buffer)
            .unwrap()
            .is_none()
    );
    let object = fake
        .object("bucket", "_unityrs/run/1/outputs.0.ndjson")
        .unwrap();
    assert_eq!(object.bytes, b"{}\n");
    assert_eq!(object.content_type.as_deref(), Some("application/x-ndjson"));
}

#[test]
fn hostile_store_reflecting_each_credential_in_code_never_renders_it() {
    use std::sync::OnceLock;
    use store_support::{fake::Config, sign};
    const KEY: &str = "AccessDenied";
    const SECRET: &str = "SlowDown";
    const TOKEN: &str = "reflected-session-token";
    run(async {
        let fake = Fake::start_with(Config {
            credentials: sign::Credentials {
                access_key: KEY.into(),
                secret: SECRET.into(),
                token: Some(TOKEN.into()),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        let settings = Settings {
            bucket: "bucket",
            region: "us-east-1",
            endpoint: Some(&fake.endpoint),
            path_style: true,
        };
        let service = http::Service::new(
            tokio::runtime::Handle::current(),
            Arc::new(unityrs_batch::sys::reset::SocketTable::default()),
        )
        .unwrap();
        let connector = Connector::new(
            service,
            client::origin(&settings).unwrap(),
            Duration::from_secs(1),
        );
        let store = client::build(
            &settings,
            None,
            &store_support::credentials(KEY, SECRET, Some(TOKEN)),
            &connector,
        )
        .unwrap();
        let pool = PermitPool::new(1).unwrap();
        let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
        let stop = AtomicBool::new(false);
        let unsettled = AtomicU64::new(0);
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let req = Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(3),
        );
        let key = Path::parse("reflected").unwrap();
        let account = Account::new(1024);
        let credentials = OnceLock::new();
        credentials.set([KEY, SECRET, TOKEN]).unwrap();
        let secrets = unityrs_batch::diagnostic::Secrets {
            values: &[],
            credentials: &credentials,
        };
        for (index, value) in [KEY, SECRET, TOKEN].into_iter().enumerate() {
            fake.schedule(
                Operation::Put,
                "reflected",
                u32::try_from(index + 1).unwrap(),
                Fault::error(403, value),
            );
            let result = req
                .put(
                    &key,
                    Payload::read(&account, 1, &mut &b"x"[..]).unwrap(),
                    "image/png",
                    b"expected",
                )
                .await;
            let record = slot.record();
            let code = rows::code(record.code, &secrets);
            assert_eq!(code, "other");
            let rendered = format!("{result:?} {record:?} {code} {:?}", fake.logs());
            for credential in [KEY, SECRET, TOKEN] {
                assert!(!rendered.contains(credential), "{rendered}");
            }
        }
    });
}
