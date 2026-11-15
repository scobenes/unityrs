//! Returned write tags and pagination consistency survive the publication merge.
#![cfg(target_os = "linux")]
mod store_support;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64},
};
use store_support::{
    Harness, Reply,
    fake::{Fake, Fault, ListPagination},
    run,
};
use unityrs_batch::{
    http::{self, permit::PermitPool},
    outcomes::{Code, Outcomes},
    phase::{Account, Error},
    publish::store::{self, Expected, Source, Target, resume, verify},
    spool::{Counter, Host, workspace::Workspace},
    store::{
        Kind,
        attempt::{Tries, TrySlot},
        client::{self, Settings},
        connector::Connector,
        put::Request,
    },
};
use unityrs_testkit::{guard::DEADLOCK_GUARD, scratch::Scratch};

struct Row<'a> {
    tag: &'a [u8],
    code: Code,
    first: bool,
    done: bool,
}
impl Source for Row<'_> {
    fn current(&self) -> Result<Option<Expected<'_>>, Error> {
        Ok((!self.done).then_some(Expected {
            key: if self.first { "a" } else { "b" },
            target: Target::Loose(0),
            code: self.code,
            etag: self.tag,
            size: 9,
        }))
    }
    fn advance(&mut self) -> Result<(), Error> {
        self.done = true;
        Ok(())
    }
}
struct Scene {
    _scratch: Scratch,
    workspace: Workspace,
    out: Outcomes,
    account: Account,
}
impl Scene {
    fn new(code: Code) -> Self {
        let scratch = Scratch::new("store-findings");
        let counter = Counter::new(1 << 24, 0, 4096).unwrap();
        let workspace = Workspace::create(
            scratch.path().join("merge"),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let out =
            Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false).unwrap();
        out.set(0, code).unwrap();
        Self {
            _scratch: scratch,
            workspace,
            out,
            account: Account::new(64 << 20),
        }
    }
}
#[test]
fn outcome_seven_verifies_size_and_the_returned_write_tag() {
    for (returned, listed, size, matches) in [
        ("\"abcdef-2\"", "abcdef-3", 9, false),
        ("\"abcdef-2\"", "ABCDEF-2", 9, true),
        ("\"opaque\"", "opaque", 9, true),
        ("\"opaque\"", "different", 9, false),
        ("\"opaque-G\"", "\"opaque-g\"", 9, false),
        ("\"opaque-g\"", "\"opaque-G\"", 9, false),
        ("\"opaque\"", "opaque", 8, false),
        ("", "anything", 9, true),
        ("", "anything", 8, false),
    ] {
        let body = format!(
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>b</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{listed}</ETag><Size>{size}</Size></Contents></ListBucketResult>"
        );
        let headers = if returned.is_empty() {
            String::new()
        } else {
            format!("ETag: {returned}\r\n")
        };
        let h = Harness::new([
            Reply::status(200, &headers, b""),
            Reply::status(200, "", body.as_bytes()),
        ]);
        let scene = Scene::new(Code::WrittenBySize);
        run(async {
            let tries = h.tries();
            let request = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + DEADLOCK_GUARD,
            );
            let cx = store::Context {
                request: &request,
                account: &scene.account,
                workspace: &scene.workspace,
                prefix: "",
                stopped: &|| false,
            };
            let written = request
                .bytes(
                    &object_store::path::Path::from("b"),
                    hyper::body::Bytes::from_static(b"123456789"),
                    "image/png",
                    b"our-digest",
                )
                .await
                .unwrap();
            assert!(written.verify_by_size);
            assert_eq!(written.etag.as_bytes(), returned.as_bytes());
            let mut row = Row {
                tag: written.etag.as_bytes(),
                code: Code::WrittenBySize,
                first: false,
                done: false,
            };
            let report = verify::run(&cx, &mut row, &scene.out, 1, 0).await.unwrap();
            assert_eq!(
                report.verified,
                Some(matches),
                "{returned} / {listed} / {size}"
            );
            assert_eq!(report.mismatches, u64::from(!matches));
            assert_eq!(report.verified_by_size, u64::from(matches));
            assert!(!report.verify_list_failed);
        });
        assert_eq!(
            scene.out.loose(0).unwrap(),
            if matches {
                Code::WrittenBySize
            } else {
                Code::PublishMismatch
            }
        );
    }
}
#[test]
fn truncated_without_a_token_retries_and_discards_both_merges() {
    inconsistent_pagination(ListPagination::TruncatedWithoutToken);
}
#[test]
fn nil_token_on_a_truncated_page_retries_and_discards_both_merges() {
    inconsistent_pagination(ListPagination::NilTokenOnTruncatedPage);
}
#[test]
fn resume_reputs_outcome_seven_objects_using_the_fresh_digest() {
    for returned in ["opaque", "abcdef-2"] {
        let body = format!(
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>b</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{returned}</ETag><Size>9</Size></Contents></ListBucketResult>"
        );
        let headers = format!("ETag: {returned}\r\n");
        let h = Harness::new([
            Reply::status(200, &headers, b""),
            Reply::status(200, "", body.as_bytes()),
            Reply::status(200, &headers, b""),
        ]);
        let scene = Scene::new(Code::NotWritten);
        run(async {
            let tries = h.tries();
            let request = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + DEADLOCK_GUARD,
            );
            let key = object_store::path::Path::from("b");
            let bytes = hyper::body::Bytes::from_static(b"123456789");
            let digest = b"25f9e794323b453885f5181f1b624d0b";
            assert!(
                request
                    .bytes(&key, bytes.clone(), "image/png", digest)
                    .await
                    .unwrap()
                    .verify_by_size
            );
            let cx = store::Context {
                request: &request,
                account: &scene.account,
                workspace: &scene.workspace,
                prefix: "",
                stopped: &|| false,
            };
            let mut row = Row {
                tag: digest,
                code: Code::NotWritten,
                first: false,
                done: false,
            };
            let report = resume::run(&cx, &mut row, &scene.out, &mut [])
                .await
                .unwrap();
            assert_eq!(report.present, 0);
            assert!(!report.resume_list_failed);
            assert_eq!(scene.out.loose(0).unwrap(), Code::NotWritten);
            assert!(
                request
                    .bytes(&key, bytes, "image/png", digest)
                    .await
                    .unwrap()
                    .verify_by_size
            );
        });
        assert_eq!(h.script.seen.count(), 3);
        assert!(h.script.seen.line(0).starts_with("PUT "));
        assert!(h.script.seen.line(2).starts_with("PUT "));
    }
}
#[test]
fn token_on_a_final_page_retries_and_discards_both_merges() {
    inconsistent_pagination(ListPagination::TokenOnFinalPage);
}
fn inconsistent_pagination(fault: ListPagination) {
    for verifying in [false, true] {
        run(async {
            let fake = Fake::start().await;
            fake.seed("bucket", "a", b"x");
            fake.seed("bucket", "b", b"123456789");
            // Stage a hit/mismatch on page one, then fail page two.
            fake.schedule_list(
                "",
                None,
                1,
                Fault {
                    list_page_size: Some(1),
                    ..Default::default()
                },
            );
            for ordinal in [1, 2] {
                fake.schedule_list(
                    "",
                    Some("offset:1"),
                    ordinal,
                    Fault {
                        list_pagination: Some(fault),
                        list_page_size: (fault == ListPagination::NilTokenOnTruncatedPage)
                            .then_some(0),
                        list_token: (fault == ListPagination::TokenOnFinalPage)
                            .then(|| "unexpected".into()),
                        ..Default::default()
                    },
                );
            }
            let (store, _pool, slot) = fake_store(&fake);
            let stop = AtomicBool::new(false);
            let unsettled = AtomicU64::new(0);
            let tries = Tries {
                policy: store_support::policy(2),
                control: &stop,
                unsettled: &unsettled,
            };
            let request = Request::new(
                &store,
                &tries,
                &slot,
                tokio::time::Instant::now() + DEADLOCK_GUARD,
            );
            let code = if verifying {
                Code::Written
            } else {
                Code::NotWritten
            };
            let scene = Scene::new(code);
            let cx = store::Context {
                request: &request,
                account: &scene.account,
                workspace: &scene.workspace,
                prefix: "",
                stopped: &|| false,
            };
            let tag = if verifying {
                b"wrong".as_slice()
            } else {
                b"9dd4e461268c8034f5c8564e155c67a6"
            };
            let mut row = Row {
                tag,
                code,
                first: true,
                done: false,
            };
            if verifying {
                let report = verify::run(&cx, &mut row, &scene.out, 1, 0).await.unwrap();
                assert_eq!(report.verified, Some(false));
                assert!(report.verify_list_failed);
                assert_eq!(report.mismatches, 0);
            } else {
                let report = resume::run(&cx, &mut row, &scene.out, &mut [])
                    .await
                    .unwrap();
                assert!(report.resume_list_failed);
                assert_eq!(report.present, 0);
            }
            assert_eq!(scene.out.loose(0).unwrap(), code);
            assert_eq!(fake.logs().len(), 3);
            assert_eq!(fake.logs()[1].raw_query, fake.logs()[2].raw_query);
            assert_eq!(scene.account.snapshot().input.used, 0);
            assert_eq!(scene.workspace.claimed(), 0);
        });
    }
}

fn fake_store(fake: &Fake) -> (client::Store, PermitPool, Arc<TrySlot>) {
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&fake.endpoint),
        path_style: true,
    };
    let service = http::Service::new(
        tokio::runtime::Handle::current(),
        Arc::new(unityrs_batch::sys::reset::SocketTable::new()),
    )
    .unwrap();
    let connector = Connector::new(service, client::origin(&settings).unwrap(), DEADLOCK_GUARD);
    let store = client::build(
        &settings,
        None,
        &store_support::credentials("AKIDEXAMPLE", "secret", None),
        &connector,
    )
    .unwrap();
    let pool = PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::ListPage);
    (store, pool, slot)
}
