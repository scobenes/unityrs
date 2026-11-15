//! Row family writes remain single PUTs and preserve the chunker's ownership.
mod store_support;
use object_store::path::Path;
use std::{sync::OnceLock, time::Duration};
use store_support::{Harness, Reply};
use unityrs_batch::{
    manifest::{Family, chunks::Destination as _},
    phase::{Account, Buffer},
    store::{rows, scan::CodeScan},
};

#[test]
fn an_owned_row_chunk_moves_to_the_request_and_releases_its_phase_charge() {
    let h = Harness::new([Reply::status(200, "ETag: \"row\"\r\n", b"")]);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let tries = h.tries();
    let req = unityrs_batch::store::put::Request::new(
        &h.store,
        &tries,
        &h.slot,
        tokio::time::Instant::now() + Duration::from_secs(2),
    );
    let account = Account::new(unityrs_core::limits::heap_cost(16 * 1024 * 1024));
    let mut bytes = Buffer::new(account.meter(), 16 * 1024 * 1024).unwrap();
    bytes.extend(b"{}\n").unwrap();
    let mut destination = rows::Destination {
        runtime: &rt,
        request: req,
        account: &account,
        prefix: "p/",
        run_id: "run",
        attempt: 3,
    };
    assert!(
        destination
            .publish_owned(Family::Diagnostics, 2, bytes)
            .unwrap()
            .is_none()
    );
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(
        h.script.seen.line(0),
        "PUT /bucket/p/_unityrs/run/3/diagnostics.2.ndjson HTTP/1.1"
    );
    assert_eq!(
        h.script.seen.header(0, "content-type").as_deref(),
        Some("application/x-ndjson")
    );
    assert_eq!(h.unsettled(), 0);
    let _ = Path::parse("p/_unityrs/run/3/diagnostics.2.ndjson").unwrap();
}

#[test]
fn code_rendering_is_closed_and_never_reflects_a_configured_credential() {
    let credentials = OnceLock::new();
    credentials
        .set(["AccessDenied", "reflected-secret", "reflected-token"])
        .unwrap();
    let secrets = unityrs_batch::diagnostic::Secrets {
        values: &[],
        credentials: &credentials,
    };
    for (raw, want) in [
        ("SlowDown", "SlowDown"),
        ("AccessDenied", "other"),
        ("reflected-secret", "other"),
        ("reflected-token", "other"),
        ("hostile-arbitrary-value", "other"),
    ] {
        let mut scan = CodeScan::default();
        scan.feed(format!("<Error><Code>{raw}</Code></Error>").as_bytes());
        assert_eq!(rows::code(scan.code(), &secrets), want);
    }
}
