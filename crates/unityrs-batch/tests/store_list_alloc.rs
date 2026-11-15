//! Count the real page operation against the charge taken by production.
#[path = "store_support/replay.rs"]
mod replay;
mod store_support;
use std::{fmt::Write, sync::Arc, time::Duration};
use unityrs_batch::{
    alloc::{self, Counting},
    http::permit::PermitPool,
    phase::Account,
    store::{
        Kind, LIST_CAP,
        attempt::{Tries, TrySlot},
        client::{self, Settings},
        connector::Connector,
        list::Listing,
        put::Request,
    },
};
#[global_allocator]
static ALLOCATOR: Counting = Counting(alloc::NoHook);

fn full_page(escaped: bool) -> String {
    let mut xml = String::from("<ListBucketResult>");
    // 64 root-level starts, followed by exactly 16 starts per Contents.
    xml.push_str(&"<n/>".repeat(63));
    for i in 0..1000 {
        write!(xml,"<Contents><Key>{i:04}{}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{}</ETag><Size>1</Size>",if escaped {"&#x41;".repeat(1020)}else{"a".repeat(1020)},"A".repeat(100)).unwrap();
        for _ in 0..11 {
            write!(xml, "<n>{}</n>", "x".repeat(if escaped { 30 } else { 520 })).unwrap();
        }
        xml.push_str("</Contents>");
    }
    xml.push_str("</ListBucketResult>");
    assert!(xml.len() as u64 <= LIST_CAP);
    assert!(xml.len() > 6 * 1024 * 1024);
    xml
}
#[test]
fn near_cap_escaped_and_skipped_event_pages_fit_the_production_reservation() {
    for (escaped, chunked) in [(false, false), (true, false), (false, true), (true, true)] {
        let xml = full_page(escaped);
        let mut reply = Vec::new();
        if chunked {
            reply.extend_from_slice(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
            for chunk in xml.as_bytes().chunks(65536) {
                reply.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                reply.extend_from_slice(chunk);
                reply.extend_from_slice(b"\r\n");
            }
            reply.extend_from_slice(b"0\r\n\r\n");
        } else {
            reply.extend_from_slice(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", xml.len()).as_bytes(),
            );
            reply.extend_from_slice(xml.as_bytes());
        }
        let url = store_support::endpoint();
        let settings = Settings {
            bucket: "bucket",
            region: "us-east-1",
            endpoint: Some(&url),
            path_style: true,
        };
        let backing: Arc<[u8]> = reply.into();
        let connector = Connector::new(
            replay::Replay(backing.clone()),
            client::origin(&settings).unwrap(),
            Duration::from_secs(10),
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
        let stop = std::sync::atomic::AtomicBool::new(false);
        let unsettled = std::sync::atomic::AtomicU64::new(0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let account = Account::new(64 << 20);
        let listing = Listing::new("").unwrap();
        let tries = Tries {
            policy: store_support::policy(1),
            control: &stop,
            unsettled: &unsettled,
        };
        let req = Request::new(
            &store,
            &tries,
            &slot,
            tokio::time::Instant::now() + Duration::from_secs(30),
        );
        let (result, measured) = alloc::measured(|| runtime.block_on(listing.page(&req, &account)));
        let page = result.unwrap().unwrap();
        assert_eq!(page.entries().len(), 1000);
        let charged = account.snapshot().input.used;
        assert!(
            measured.peak <= charged,
            "peak {} exceeds production charge {charged}",
            measured.peak
        );
        eprintln!(
            "LIST escaped={escaped} chunked={chunked}: body {}, tags 16064, counted peak {}, production charge {charged}",
            xml.len(),
            measured.peak
        );
        drop(page);
        assert_eq!(account.snapshot().input.used, 0);
        drop(slot.take_permit());
        assert_eq!(pool.snapshot().occupied, 0);
        drop(backing); // Peer storage never entered or left the counted window.
    }
}
