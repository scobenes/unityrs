//! Real loopback TLS ownership, trust and pre-connect closing checks.
#![cfg(feature = "test-hooks")]
mod http_support;
use http_support::{
    run,
    tls::{Peer, Table},
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use unityrs_batch::http::{self, Detail, Payload, permit::PermitPool};
use unityrs_testkit::guard::DEADLOCK_GUARD;

#[test]
fn compiled_roots_reject_a_private_ca_and_test_roots_only_enter_through_the_hook() {
    let _serial = http_support::tls::lock();
    for roots in [false, true] {
        let peer = Peer::tls(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
            roots,
        );
        let table = Arc::new(Table::default());
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        let url = url::Url::parse("https://localhost/secret?new-secret=value").unwrap();
        run(async {
            let service = table.service(tokio::runtime::Handle::current(), &[peer.address], roots);
            let request = http::prepare(
                &url,
                hyper::http::Method::GET,
                hyper::http::HeaderMap::new(),
                Payload::default(),
            )
            .unwrap();
            let result = service
                .request(&mut permit, &url, request, DEADLOCK_GUARD)
                .await;
            if roots {
                let mut response = result.ok().unwrap();
                assert_eq!(response.status(), 200);
                assert_eq!(response.data().await.unwrap().unwrap().as_slice(), b"ok");
                assert!(response.data().await.unwrap().is_none());
                drop(response);
            } else {
                assert_eq!(result.err().unwrap().detail(), Some(Detail::Tls));
            }
        });
        let observation = peer.finish();
        if roots {
            let observation = observation.unwrap();
            assert_eq!(observation.alpn.as_deref(), Some(b"http/1.1".as_slice()));
            assert!(
                observation
                    .request
                    .starts_with(b"GET /secret?new-secret=value HTTP/1.1\r\n")
            );
            assert!(observation.reset);
            assert!(
                !observation.close_notify,
                "abortive close must bypass TLS shutdown"
            );
        } else {
            assert!(observation.is_err());
        }
        assert_eq!(table.added.load(Ordering::Acquire), 1);
        assert_eq!(table.removed.load(Ordering::Acquire), 1);
    }
}

#[test]
fn dropping_an_unread_response_resets_and_unregisters_its_only_socket() {
    let _serial = http_support::tls::lock();
    let peer = Peer::tls(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx".to_vec(),
        true,
    );
    let table = Arc::new(Table::default());
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = table.service(tokio::runtime::Handle::current(), &[peer.address], true);
        let url = url::Url::parse("https://localhost/").unwrap();
        let response = service
            .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
            .await
            .ok()
            .unwrap();
        assert_eq!(table.removed.load(Ordering::Acquire), 0);
        drop(response);
        assert_eq!(table.removed.load(Ordering::Acquire), 1);
    });
    assert!(peer.finish().unwrap().reset);
}

#[test]
fn registration_precedes_the_closing_load_and_no_connect_follows_it() {
    let _serial = http_support::tls::lock();
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let table = Arc::new(Table::default());
    table.close_on_register.store(true, Ordering::Release);
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = table.service(
            tokio::runtime::Handle::current(),
            &[listener.local_addr().unwrap()],
            false,
        );
        let url = url::Url::parse("http://localhost/").unwrap();
        let error = service
            .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
            .await
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::Connect));
    });
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(table.removed.load(Ordering::Acquire), 1);
}

#[test]
fn resolver_errors_and_a_connection_timer_are_connect_failures() {
    use unityrs_batch::http::{
        Service,
        resolve::{Addresses, Resolver},
    };
    struct Refuse;
    impl Resolver for Refuse {
        fn resolve(&self, _: &str, _: u16) -> std::io::Result<Addresses> {
            Err(std::io::ErrorKind::NotFound.into())
        }
    }
    let _serial = http_support::tls::lock();
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = Service::new(
            tokio::runtime::Handle::current(),
            Arc::new(Table::default()),
        )
        .unwrap()
        .with_test_hooks(Arc::new(Refuse), &[])
        .unwrap();
        let url = url::Url::parse("http://localhost/").unwrap();
        let error = service
            .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
            .await
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::Connect));
        let error = service
            .request(&mut permit, &url, http_support::get(), Duration::ZERO)
            .await
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::Connect));
    });
}

#[test]
fn timed_out_blocking_lookup_keeps_its_slot_while_another_permit_progresses() {
    use std::sync::{Mutex, mpsc};
    use unityrs_batch::http::{
        Service,
        resolve::{Addresses, Resolver},
    };
    use unityrs_testkit::guard;
    struct Hold {
        started: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Resolver for Hold {
        fn resolve(&self, _: &str, _: u16) -> std::io::Result<Addresses> {
            self.started.send(()).unwrap();
            guard::hold(&self.release.lock().unwrap(), "release held resolver");
            Ok(Addresses::default())
        }
    }
    let _serial = http_support::tls::lock();
    let (started, began) = mpsc::sync_channel(1);
    let (release, held) = mpsc::sync_channel(1);
    let resolver = Arc::new(Hold {
        started,
        release: Mutex::new(held),
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(3)
        .build()
        .unwrap();
    let pool = PermitPool::new(2).unwrap();
    let mut slow = pool.acquire().unwrap();
    let service = Service::new(runtime.handle().clone(), Arc::new(Table::default()))
        .unwrap()
        .with_test_hooks(resolver, &[])
        .unwrap();
    let url = url::Url::parse("http://localhost/").unwrap();
    let error = runtime.block_on(async {
        tokio::time::timeout(
            DEADLOCK_GUARD,
            service.request(&mut slow, &url, http_support::get(), Duration::ZERO),
        )
        .await
        .expect("deadlock guard")
        .err()
        .unwrap()
    });
    assert_eq!(error.detail(), Some(Detail::Connect));
    let () = guard::recv(&began, "lookup began");
    assert_eq!(slow.charged(), http::resolve::LOOKUP_ENVELOPE);
    drop(slow);
    assert_eq!(pool.snapshot().outstanding, 1);
    let fast = pool.acquire().unwrap();
    assert!(pool.acquire().is_none());
    let mut response = runtime
        .block_on(http::exchange(
            http_support::MemoryIo::new(
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
                1,
            ),
            &fast,
            http_support::get(),
        ))
        .ok()
        .unwrap();
    assert!(runtime.block_on(response.data()).unwrap().is_none());
    drop((response, fast));
    release.send(()).unwrap();
    // Runtime shutdown explicitly quiesces the now-released closure.
    drop(runtime);
    assert_eq!(pool.snapshot().outstanding, 0);
    assert_eq!(pool.snapshot().occupied, 0);
}

#[test]
fn at_most_sixteen_addresses_connect_and_failed_sockets_are_removed() {
    let _serial = http_support::tls::lock();
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut addresses = vec![std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)); 16];
    addresses.push(listener.local_addr().unwrap());
    let table = Arc::new(Table::default());
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = table.service(tokio::runtime::Handle::current(), &addresses, false);
        let url = url::Url::parse("http://localhost/").unwrap();
        let error = service
            .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
            .await
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::Connect));
    });
    assert_eq!(table.added.load(Ordering::Acquire), 16);
    assert_eq!(table.removed.load(Ordering::Acquire), 16);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn trusted_certificates_still_require_the_correct_name_and_valid_time() {
    let _serial = http_support::tls::lock();
    for leaf in [http_support::tls::WRONG_HOST, http_support::tls::EXPIRED] {
        let peer = Peer::certificate(Vec::new(), leaf);
        let table = Arc::new(Table::default());
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        run(async {
            let service = table.service(tokio::runtime::Handle::current(), &[peer.address], true);
            let url = url::Url::parse("https://localhost/").unwrap();
            let error = service
                .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
                .await
                .err()
                .unwrap();
            assert_eq!(error.detail(), Some(Detail::Tls));
        });
        assert!(peer.finish().is_err());
        assert_cleanup(&table, &pool, permit, 1);
    }
}

#[test]
fn an_ip_url_verifies_the_ip_san_without_sending_sni() {
    let _serial = http_support::tls::lock();
    let peer = Peer::tls(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
        true,
    );
    let table = Arc::new(Table::default());
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = table.service(tokio::runtime::Handle::current(), &[peer.address], true);
        let url = url::Url::parse("https://127.0.0.1/").unwrap();
        let request = http::prepare(
            &url,
            hyper::http::Method::GET,
            hyper::http::HeaderMap::new(),
            Payload::default(),
        )
        .unwrap();
        let mut response = service
            .request(&mut permit, &url, request, DEADLOCK_GUARD)
            .await
            .ok()
            .unwrap();
        assert_eq!(response.data().await.unwrap().unwrap().as_slice(), b"ok");
        assert!(response.data().await.unwrap().is_none());
    });
    let observation = peer.finish().unwrap();
    assert!(observation.server_name.is_none());
    assert!(observation.reset);
    assert!(!observation.close_notify);
    assert_cleanup(&table, &pool, permit, 1);
}

#[test]
fn handshake_eof_reset_and_stall_are_connect_failures_and_release_ownership() {
    use http_support::tls::Handshake;
    let _serial = http_support::tls::lock();
    for mode in [Handshake::Eof, Handshake::Reset, Handshake::Stall] {
        let peer = Peer::handshake(mode);
        let table = Arc::new(Table::default());
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        run(async {
            let service = table.service(tokio::runtime::Handle::current(), &[peer.address], true);
            let url = url::Url::parse("https://localhost/").unwrap();
            let timeout = if matches!(mode, Handshake::Stall) {
                Duration::from_millis(100)
            } else {
                DEADLOCK_GUARD
            };
            let error = service
                .request(&mut permit, &url, http_support::get(), timeout)
                .await
                .err()
                .unwrap();
            assert_eq!(error.detail(), Some(Detail::Connect), "{mode:?}");
        });
        let observation = peer.finish().unwrap();
        if matches!(mode, Handshake::Stall) {
            assert!(observation.reset);
        }
        assert_cleanup(&table, &pool, permit, 1);
    }
}

#[test]
fn second_client_hello_contains_no_psk_or_session_ticket() {
    let _serial = http_support::tls::lock();
    let peers = Peer::twice(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    let table = Arc::new(Table::default());
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    run(async {
        let service = table.service(tokio::runtime::Handle::current(), &[peers[0].address], true);
        let url = url::Url::parse("https://localhost/").unwrap();
        for (index, peer) in peers.into_iter().enumerate() {
            let mut response = service
                .request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD)
                .await
                .ok()
                .unwrap();
            assert_eq!(response.data().await.unwrap().unwrap().as_slice(), b"ok");
            assert!(response.data().await.unwrap().is_none());
            drop(response);
            let observation = peer.finish().unwrap();
            let extensions = client_hello_extensions(&observation.client_hello);
            if index == 1 {
                assert!(
                    !extensions.contains(&41),
                    "second ClientHello pre_shared_key"
                );
                assert!(
                    !extensions.contains(&35),
                    "second ClientHello session_ticket"
                );
                assert!(!extensions.contains(&42), "second ClientHello early_data");
            }
            assert!(observation.reset);
            assert!(!observation.close_notify);
        }
    });
    assert_cleanup(&table, &pool, permit, 2);
}

fn assert_cleanup(
    table: &Table,
    pool: &PermitPool,
    permit: unityrs_batch::http::permit::RequestPermit,
    sockets: usize,
) {
    assert_eq!(table.added.load(Ordering::Acquire), sockets);
    assert_eq!(table.removed.load(Ordering::Acquire), sockets);
    assert_eq!(permit.charged(), 0);
    drop(permit);
    assert_eq!(pool.snapshot().outstanding, 0);
    assert_eq!(pool.snapshot().occupied, 0);
    assert!(pool.acquire().is_some());
}

fn client_hello_extensions(record: &[u8]) -> Vec<u16> {
    fn u16_at(bytes: &[u8], offset: usize) -> usize {
        usize::from(u16::from_be_bytes(
            bytes[offset..offset + 2].try_into().unwrap(),
        ))
    }
    assert_eq!(record[0], 22, "TLS handshake record");
    assert_eq!(record[5], 1, "ClientHello");
    let end = 5 + u16_at(record, 3);
    assert!(record.len() >= end);
    let mut offset = 9 + 2 + 32;
    offset += 1 + usize::from(record[offset]); // legacy session id
    offset += 2 + u16_at(record, offset); // cipher suites
    offset += 1 + usize::from(record[offset]); // compression methods
    assert_eq!(offset + 2 + u16_at(record, offset), end);
    offset += 2;
    let mut extensions = Vec::new();
    while offset < end {
        extensions.push(u16::try_from(u16_at(record, offset)).unwrap());
        offset += 4 + u16_at(record, offset + 2);
    }
    assert_eq!(offset, end);
    extensions
}
