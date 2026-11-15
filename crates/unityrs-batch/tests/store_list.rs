//! LIST pages are charged through their last consumer, never prefetched.
mod store_support;
use std::time::Duration;
use store_support::{Harness, Reply, run};
use unityrs_batch::{
    phase::Account,
    store::{
        list::{Error, Listing, RESERVATION},
        put::Request,
    },
};

fn page(key: &str, size: u64, token: &str) -> Reply {
    let token = format!("<IsTruncated>{}</IsTruncated>{token}", !token.is_empty());
    Reply::status(200, "", format!("<ListBucketResult><Contents><Key>{key}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>&quot;ABC&quot;</ETag><Size>{size}</Size></Contents>{token}</ListBucketResult>").as_bytes())
}
#[test]
fn pages_hold_the_reservation_and_send_the_exact_prefix_and_token() {
    let h = Harness::new([
        page(
            "p/a",
            1,
            "<NextContinuationToken>a&amp;b</NextContinuationToken>",
        ),
        page("p/b", 2, ""),
    ]);
    let account = Account::new(64 << 20);
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let listing = Listing::new("p/").unwrap();
        let first = listing.page(&req, &account).await.unwrap().unwrap();
        assert_eq!(first.entries().len(), 1);
        assert_eq!(first.entries()[0].location.as_ref(), "p/a");
        assert_eq!(account.snapshot().input.used, RESERVATION);
        assert_eq!(
            listing.page(&req, &account).await.err(),
            Some(Error::PageHeld)
        );
        drop(first);
        let second = listing.page(&req, &account).await.unwrap().unwrap();
        assert_eq!(second.entries()[0].size, 2);
        drop(second);
        assert!(listing.page(&req, &account).await.unwrap().is_none());
    });
    let first = h.script.seen.line(0);
    assert!(first.contains("prefix=p%2F"), "{first}");
    assert!(first.contains("max-keys=1000"), "{first}");
    assert!(h.script.seen.line(1).contains("continuation-token=a%26b"));
    assert_eq!(h.unsettled(), 0);
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn a_late_out_of_order_page_fails_but_duplicate_zero_size_markers_are_skipped() {
    for (size, fails) in [(0, false), (1, true)] {
        let h = Harness::new([
            page(
                "p/a",
                1,
                "<NextContinuationToken>next</NextContinuationToken>",
            ),
            page("p/a/", size, ""),
        ]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let listing = Listing::new("p/").unwrap();
            drop(listing.page(&req, &account).await.unwrap());
            let second = listing.page(&req, &account).await;
            if fails {
                assert_eq!(second.err(), Some(Error::Order));
            } else {
                assert_eq!(second.unwrap().unwrap().entries(), []);
            }
        });
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn oversize_tokens_and_etags_are_retryable_2xx_errors() {
    for (key, token) in [
        (
            "p/a",
            format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                "t".repeat(1025)
            ),
        ),
        ("p/a", "<NextContinuationToken/>".into()),
    ] {
        let h = Harness::new([page(key, 1, &token)]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let listing = Listing::new("p/").unwrap();
            assert_eq!(
                listing.page(&req, &account).await.err(),
                Some(Error::Failed)
            );
        });
        assert_eq!(h.unsettled(), 0);
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn malformed_pages_use_the_try_table_and_never_count_as_unsettled_writes() {
    for bad in [
        Reply::error(200, "InternalError"),
        Reply::status(200, "", b"<ListBucketResult><Contents>"),
        page(
            "p/a",
            1,
            &format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                "t".repeat(1025)
            ),
        ),
    ] {
        let mut h = Harness::new([bad, page("p/a", 1, "")]);
        h.policy.attempts = 2;
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            drop(
                Listing::new("p/")
                    .unwrap()
                    .page(&req, &account)
                    .await
                    .unwrap(),
            );
        });
        assert_eq!(h.script.seen.count(), 2);
        assert_eq!(h.unsettled(), 0);
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn reservation_busy_and_refused_are_distinct_and_send_nothing() {
    let h = Harness::new([]);
    let account = Account::new(RESERVATION);
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let charge = account.meter().charge(1).unwrap();
        let listing = Listing::new("").unwrap();
        assert_eq!(listing.page(&req, &account).await.err(), Some(Error::Busy));
        drop(charge);
        assert_eq!(
            listing
                .page(&req, &Account::new(RESERVATION - 1))
                .await
                .err(),
            Some(Error::Reservation)
        );
        assert_eq!(account.snapshot().refusals(), 0);
    });
    assert_eq!(h.script.seen.count(), 0);
}
#[test]
fn the_zero_size_prefix_folder_marker_is_ignored() {
    let h = Harness::new([page("p/", 0, "")]);
    let account = Account::new(64 << 20);
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let listing = Listing::new("p/").unwrap();
        let page = listing.page(&req, &account).await.unwrap().unwrap();
        assert_eq!(page.entries(), []);
    });
}

#[test]
#[allow(clippy::too_many_lines)] // One socket scene pins page retries and their independent ordinals.
fn per_page_fake_faults_retry_the_same_token_and_preserve_signatures() {
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use store_support::fake::{Fake, Fault};
    use unityrs_batch::{
        http::{self, permit::PermitPool},
        store::{
            Kind,
            attempt::{Tries, TrySlot},
            client::{self, Settings},
            connector::Connector,
        },
    };
    run(async {
        let fake = Fake::start().await;
        fake.seed("bucket", "p/", b"");
        for key in ["p/a", "p/a/", "p/b", "p/雪%# ~"] {
            fake.seed("bucket", key, if key.ends_with('/') { b"" } else { b"x" });
        }
        fake.seed("bucket", "outside", b"x");
        fake.schedule_list(
            "p/",
            None,
            1,
            Fault {
                list_page_size: Some(2),
                ..Default::default()
            },
        );
        fake.schedule_list("p/", Some("offset:2"), 1, Fault::error(503, "SlowDown"));
        let settings = Settings {
            bucket: "bucket",
            region: "us-east-1",
            endpoint: Some(&fake.endpoint),
            path_style: true,
        };
        let service = http::Service::new(
            tokio::runtime::Handle::current(),
            std::sync::Arc::new(unityrs_batch::sys::reset::SocketTable::new()),
        )
        .unwrap();
        let connector = Connector::new(
            service,
            client::origin(&settings).unwrap(),
            Duration::from_secs(2),
        );
        let store = client::build(
            &settings,
            None,
            &store_support::credentials("AKIDEXAMPLE", "secret", None),
            &connector,
        )
        .unwrap();
        let pool = PermitPool::new(1).unwrap();
        let slot = TrySlot::new(pool.acquire().unwrap(), Kind::ListPage);
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
        let account = Account::new(64 << 20);
        let listing = Listing::new("p/").unwrap();
        let first = listing.page(&req, &account).await.unwrap().unwrap();
        assert_eq!(first.entries().len(), 1); // p/a/ normalizes to duplicate p/a.
        drop(first);
        let second = listing.page(&req, &account).await.unwrap().unwrap();
        assert_eq!(second.entries().len(), 2);
        assert_eq!(second.entries()[1].location.as_ref(), "p/雪%# ~");
        drop(second);
        assert!(listing.page(&req, &account).await.unwrap().is_none());
        let logs = fake.logs();
        assert_eq!(logs.len(), 3);
        assert!(logs.iter().all(|l| l.signature_valid));
        assert_eq!(logs[1].raw_query, logs[2].raw_query);
        assert_eq!(logs[1].try_ordinal, 1);
        assert_eq!(logs[2].try_ordinal, 2);
        fake.schedule_list(
            "p/",
            None,
            2,
            Fault {
                list_reverse: true,
                ..Default::default()
            },
        );
        assert_eq!(
            Listing::new("p/").unwrap().page(&req, &account).await.err(),
            Some(Error::Order)
        );
        for ordinal in [3, 4] {
            fake.schedule_list(
                "p/",
                None,
                ordinal,
                Fault {
                    list_token: Some("t".repeat(1025)),
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            Listing::new("p/").unwrap().page(&req, &account).await.err(),
            Some(Error::Failed)
        );
        drop(slot.take_permit());
        assert_eq!(pool.snapshot().occupied, 0);
    });
}
