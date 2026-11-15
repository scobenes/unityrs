//! Drop every IO suspension of page reads, retry waits and continuation calls.
mod store_support;
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use store_support::{Harness, Reply, Script, run};
use unityrs_batch::{
    phase::Account,
    store::{
        client::{self, Settings},
        connector::Connector,
        list::Listing,
        put::Request,
    },
};
fn yielding(replies: Vec<Reply>) -> Harness {
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
    h.policy.attempts = 2;
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
fn page(key: &str, token: &str) -> Reply {
    let token = format!("<IsTruncated>{}</IsTruncated>{token}", !token.is_empty());
    Reply::status(200,"",format!("<ListBucketResult><Contents><Key>{key}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>abc</ETag><Size>1</Size></Contents>{token}</ListBucketResult>").as_bytes())
}
#[test]
fn every_io_yield_returns_page_charges_and_permits_on_drop() {
    for retrying in [false, true] {
        let mut finished = false;
        for at in 0..256 {
            let mut replies = Vec::new();
            if retrying {
                replies.push(Reply::error(503, "SlowDown"));
            }
            replies.push(page(
                "a",
                "<NextContinuationToken>next</NextContinuationToken>",
            ));
            replies.push(page("b", ""));
            let h = yielding(replies);
            let account = Account::new(64 << 20);
            let acknowledged = run(async {
                let tries = h.tries();
                let req = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                );
                let listing = Listing::new("").unwrap();
                cut(at, async {
                    while let Some(page) = listing.page(&req, &account).await.unwrap() {
                        assert_eq!(page.entries().len(), 1);
                        drop(page);
                    }
                })
                .await
                .is_some()
            });
            assert_eq!(account.snapshot().input.used, 0, "cut {at}");
            assert_eq!(h.unsettled(), 0);
            drop(h.slot.take_permit());
            assert_eq!(h.pool.snapshot().occupied, 0, "cut {at}");
            if acknowledged {
                finished = true;
                break;
            }
        }
        assert!(finished, "sweep must reach completion");
    }
}
