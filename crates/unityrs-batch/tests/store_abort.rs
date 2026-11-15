mod store_support;
use object_store::path::Path;
use std::time::Duration;
use store_support::{Harness, Reply, run};
use unityrs_batch::{
    spool::journal::{Entry, Journal},
    store::{abort, put::Request},
};
use unityrs_testkit::scratch::Scratch;
#[test]
fn abort_no_such_upload_succeeds_without_unsettled_counting() {
    let h = Harness::new([Reply::error(404, "NoSuchUpload")]);
    run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        assert_eq!(
            abort::upload(&req, &Path::parse("k").unwrap(), &"upload".to_owned()).await,
            Ok(())
        );
    });
    assert_eq!(h.unsettled(), 0);
    assert_eq!(
        h.script.seen.line(0),
        "DELETE /bucket/k?uploadId=upload HTTP/1.1"
    );
}
#[test]
fn recovery_matches_exact_endpoint_and_bucket_and_never_fails_start() {
    let scratch = Scratch::new("store-abort");
    let journal = Journal::new(scratch.path()).unwrap();
    let entry = Entry {
        endpoint: None,
        bucket: "bucket",
        key: "k",
        upload_id: "u",
    };
    for record in [
        entry,
        Entry {
            key: "failed",
            ..entry
        },
        Entry {
            endpoint: Some("http://other.test"),
            ..entry
        },
        Entry {
            bucket: "other",
            ..entry
        },
    ] {
        journal.append(record).unwrap();
    }
    let h = Harness::new([
        Reply::status(204, "", b""),
        Reply::error(403, "AccessDenied"),
    ]);
    let report = run(async {
        let tries = h.tries();
        let req = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(2),
        );
        abort::recover_journal(&req, &journal, None, "bucket").await
    });
    assert_eq!(report.aborted, 1);
    assert_eq!(report.failed, 1);
    let mut reader = journal.reader().unwrap();
    let mut left = Vec::new();
    while let Some(record) = reader.next().unwrap() {
        left.push(record.entry().key.to_owned());
    }
    assert_eq!(left, ["failed", "k", "k"]);
    assert_eq!(h.script.seen.count(), 2);
}
