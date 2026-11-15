//! LIST error vocabulary, retry table, kept-field boundaries and order checks.
mod store_support;
use std::time::Duration;
use store_support::{Harness, Reply, run};
use unityrs_batch::{
    phase::Account,
    store::{
        list::{Error, Listing},
        put::Request,
    },
};
fn contents(key: &str, etag: &str, size: u64) -> String {
    format!(
        "<Contents><Key>{key}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{etag}</ETag><Size>{size}</Size></Contents>"
    )
}
fn page(body: &str) -> Reply {
    Reply::status(
        200,
        "",
        format!("<ListBucketResult>{body}</ListBucketResult>").as_bytes(),
    )
}
#[test]
fn status_failures_stay_closed_and_reads_never_increment_unsettled() {
    for status in [301, 400, 403, 404, 408, 429, 500, 503] {
        let h = Harness::new([Reply::error(
            status,
            if status == 403 {
                "ExpiredToken"
            } else {
                "AccessDenied"
            },
        )]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            assert_eq!(
                Listing::new("").unwrap().page(&req, &account).await.err(),
                Some(Error::Failed)
            );
        });
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(h.unsettled(), 0);
    }
}
#[test]
fn kept_etags_and_continuations_are_checked_at_the_exact_limits() {
    for length in [100, 101] {
        let h = Harness::new([page(&contents("a", &"t".repeat(length), 1))]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let listing = Listing::new("").unwrap();
            let result = listing.page(&req, &account).await;
            assert_eq!(result.is_ok(), length == 100);
        });
    }
    for length in [1024, 1025] {
        let body = format!(
            "<IsTruncated>true</IsTruncated><NextContinuationToken>{}</NextContinuationToken>",
            "t".repeat(length)
        );
        let h = Harness::new([page(&body), page("")]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let listing = Listing::new("").unwrap();
            let first = listing.page(&req, &account).await;
            assert_eq!(first.is_ok(), length == 1024);
            drop(first);
            if length == 1024 {
                drop(listing.page(&req, &account).await.unwrap());
                assert!(h.script.seen.line(1).contains(&"t".repeat(1024)));
            }
        });
    }
}
#[test]
fn nonzero_regressions_and_duplicates_fail_even_inside_a_page() {
    for body in [
        format!("{}{}", contents("b", "tag", 1), contents("a", "tag", 1)),
        format!("{}{}", contents("a", "tag", 1), contents("a", "tag", 1)),
    ] {
        let h = Harness::new([page(&body)]);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            assert_eq!(
                Listing::new("").unwrap().page(&req, &account).await.err(),
                Some(Error::Order)
            );
        });
    }
}
#[test]
fn a_stop_or_past_deadline_sends_no_list() {
    for stopped in [false, true] {
        let h = Harness::new([]);
        h.stop.store(stopped, std::sync::atomic::Ordering::Release);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() - Duration::from_millis(1),
            );
            assert_eq!(
                Listing::new("").unwrap().page(&req, &account).await.err(),
                Some(Error::Stopped)
            );
        });
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(h.script.seen.count(), 0);
    }
}
#[test]
fn connection_and_body_timeouts_retry_without_leaking_the_read_permit() {
    use std::sync::atomic::Ordering;
    use store_support::End;
    for connected in [false, true] {
        let first = if connected {
            Reply::chunks(
                vec![b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\n<ListBucketResult>".to_vec()],
                End::Stall,
            )
        } else {
            Reply::StallConnect
        };
        let mut h = Harness::new([first, page(&contents("a", "abc", 1))]);
        h.policy.attempts = 2;
        h.policy.timeout = Duration::from_millis(2);
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let req = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let listing = Listing::new("").unwrap();
            let result = listing.page(&req, &account).await.unwrap().unwrap();
            assert_eq!(result.entries().len(), 1);
        });
        assert_eq!(h.script.sent.load(Ordering::Acquire), 2);
        assert_eq!(h.unsettled(), 0);
        assert_eq!(account.snapshot().input.used, 0);
        drop(h.slot.take_permit());
        assert_eq!(h.pool.snapshot().occupied, 0);
    }
}
