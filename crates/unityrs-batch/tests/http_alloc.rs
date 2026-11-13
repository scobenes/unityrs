//! Focused network allowance gates. Peer storage is outside the counted window.
//! Include runtime/pool setup and teardown as conservative extra overhead:
//! lazy runtime state and deferred IO cleanup must not look like leaked HTTP data.
//! Thread-local runtime caches can outlive teardown, so assert that transport
//! charges return to zero rather than treating total retained heap as a leak.
#![cfg(feature = "test-hooks")]
#[path = "http_support/count.rs"]
mod count;
mod http_support;
use http_support::{
    MemoryIo,
    tls::{Peer, Table},
};
use std::sync::Arc;
use unityrs_batch::{
    alloc::Counting,
    http::{
        self, Detail, LIBRARY_ENVELOPE, Payload,
        permit::{NETWORK_ALLOWANCE, PermitPool},
    },
};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::guard::DEADLOCK_GUARD;

#[global_allocator]
static ALLOCATOR: Counting<count::Network> = Counting(count::Network);

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn largest_heads_trailers_and_backing_overlap_fit_the_network_allowance() {
    let _serial = count::lock();
    for (kind, fragment) in [
        (0, usize::MAX),
        (1, usize::MAX),
        (2, usize::MAX),
        (3, usize::MAX),
        (3, 1_024),
    ] {
        let bytes = match kind {
            0 => http_support::large_head(131_072, true),
            1 => {
                let mut bytes =
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\nx: "
                        .to_vec();
                bytes.extend(std::iter::repeat_n(b'a', 131_064));
                bytes.extend_from_slice(b"\r\n\r\n");
                bytes
            }
            2 => {
                let mut bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 524288\r\n\r\n".to_vec();
                bytes.resize(bytes.len() + 524_288, b'x');
                bytes
            }
            _ => http_support::combined_response(),
        };
        let io = MemoryIo::new(bytes, fragment);
        let backing = io.backing();
        count::start();
        let result = {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let pool = PermitPool::new(1).unwrap();
            let permit = pool.acquire().unwrap();
            let result = runtime.block_on(async {
                tokio::time::timeout(DEADLOCK_GUARD, async {
                    // Request construction is included in the counted window.
                    let mut response = http::exchange(io, &permit, http_support::get()).await?;
                    let mut last = None;
                    while let Some(data) = response.data().await? {
                        assert!(data.as_slice().len() <= 65_536);
                        count::hold_pieces(0);
                        drop(last.take());
                        count::hold_pieces(heap_cost(
                            u64::try_from(data.as_slice().len()).unwrap(),
                        ));
                        last = Some(data);
                    }
                    count::hold_pieces(0);
                    drop(last);
                    Ok::<_, http::Error>(())
                })
                .await
                .expect("deadlock guard")
            });
            assert_eq!(permit.charged(), 0);
            result
        };
        let figures = count::finish();
        drop(backing);
        result.unwrap();
        assert!(
            figures.peak <= NETWORK_ALLOWANCE,
            "scene {kind}: {figures:?}"
        );
        assert!(
            count::library_peak() <= LIBRARY_ENVELOPE,
            "scene {kind}: library={} reservation={LIBRARY_ENVELOPE}",
            count::library_peak()
        );
        println!(
            "HTTP_MEMORY scene={kind} fragment={fragment} peak={} allowance={NETWORK_ALLOWANCE}",
            figures.peak
        );
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn tls_handshake_and_streaming_fit_the_network_allowance() {
    let _serial = count::lock();
    for scene in [
        "ordinary",
        "certificates",
        "authorities",
        "malformed-combined",
        "combined-success",
        "tls12-empty",
        "tls12-one-byte",
        "tls13-minimal",
        "tls13-authorities",
        "tls12-ocsp",
        "authorities-cap32768",
        "combined-minimal",
        "maximal-request",
        "maximal-combined",
    ] {
        let response = if matches!(
            scene,
            "combined-success" | "combined-minimal" | "maximal-combined"
        ) {
            http_support::combined_response()
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
        };
        let peer = match scene {
            "certificates" => Peer::tls_large_chain(response),
            "authorities" => Peer::tls_many_authorities(response),
            "malformed-combined" => Peer::tls_stress(response),
            "ordinary" | "maximal-request" => Peer::tls(response, true),
            _ => Peer::with_aligned_config(response, http_support::stress::config(scene)),
        };
        count::start();
        {
            let table = Arc::new(Table::default());
            let pool = PermitPool::new(1).unwrap();
            let mut permit = pool.acquire().unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .on_thread_start(count::mark)
                .build()
                .unwrap();
            let service = table.service(runtime.handle().clone(), &[peer.address], true);
            let url = if matches!(scene, "maximal-request" | "maximal-combined") {
                // Full serialized URL cap; origin form includes all but the
                // scheme and authority. All 32 values are exactly 16 KiB.
                url::Url::parse(&format!("https://localhost/{}", "a".repeat(16_384 - 18))).unwrap()
            } else {
                url::Url::parse("https://localhost/").unwrap()
            };
            runtime.block_on(tls_scene(&service, &mut permit, &url, scene));
            assert_eq!(permit.charged(), 0);
            assert_eq!(pool.snapshot().outstanding, 0);
            assert_eq!(table.added.load(std::sync::atomic::Ordering::Acquire), 1);
            assert_eq!(table.removed.load(std::sync::atomic::Ordering::Acquire), 1);
            drop(permit);
            assert_eq!(pool.snapshot().occupied, 0);
        }
        let figures = count::finish();
        assert!(
            figures.peak <= NETWORK_ALLOWANCE,
            "TLS {scene}: {figures:?}"
        );
        assert!(
            count::library_peak() <= LIBRARY_ENVELOPE,
            "TLS {scene}: library={} reservation={LIBRARY_ENVELOPE}",
            count::library_peak()
        );
        println!(
            "HTTP_TLS_MEMORY scene={scene} peak={} library_peak={} reservation={LIBRARY_ENVELOPE}",
            figures.peak,
            count::library_peak()
        );
        if scene == "malformed-combined" {
            assert!(peer.finish().is_err());
        } else {
            let observation = peer.finish().unwrap();
            assert!(observation.reset);
            if matches!(scene, "maximal-request" | "maximal-combined") {
                assert!(observation.request.len() > 32 * 16_384 + 16_000);
            }
        }
    }
}

async fn tls_scene(
    service: &http::Service,
    permit: &mut http::permit::RequestPermit,
    url: &url::Url,
    scene: &str,
) {
    tokio::time::timeout(DEADLOCK_GUARD, async {
        let mut headers = hyper::http::HeaderMap::new();
        if matches!(scene, "maximal-request" | "maximal-combined") {
            for index in 0..32 {
                headers.insert(
                    hyper::http::HeaderName::from_bytes(format!("x-{index}").as_bytes()).unwrap(),
                    hyper::http::HeaderValue::from_bytes(&vec![b'a'; 16_384]).unwrap(),
                );
            }
        }
        let mut request =
            http::prepare(url, hyper::http::Method::GET, headers, Payload::default()).unwrap();
        if matches!(scene, "maximal-request" | "maximal-combined") {
            // Exercise the transport URI cap independently of the serialized
            // URL cap, which also includes its scheme and authority.
            *request.uri_mut() = format!("/{}", "a".repeat(16_383)).parse().unwrap();
        }
        let result = service.request(permit, url, request, DEADLOCK_GUARD).await;
        if scene == "malformed-combined" {
            assert!(matches!(
                result.err().unwrap().detail(),
                Some(Detail::Connect | Detail::Tls)
            ));
            return;
        }
        let mut response = result.unwrap_or_else(|error| panic!("{scene}: {error:?}"));
        let mut last = None;
        let mut length = 0;
        while let Some(data) = response.data().await.unwrap() {
            length += data.as_slice().len();
            count::hold_pieces(0);
            drop(last.take());
            count::hold_pieces(heap_cost(u64::try_from(data.as_slice().len()).unwrap()));
            last = Some(data);
        }
        if matches!(
            scene,
            "combined-success" | "combined-minimal" | "maximal-combined"
        ) {
            assert_eq!(length, 131_072);
        } else {
            assert_eq!(length, 2);
        }
        count::hold_pieces(0);
        drop(last);
    })
    .await
    .expect("deadlock guard");
}
