//! Resolver quarantine and TLS roots through the official binary hooks.
#![cfg(target_os = "linux")]
mod common;
mod scenes;
#[path = "../../unityrs-batch/tests/url_run_support/mod.rs"]
mod url_run_support;
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use scenes::{ended, job, start, traced};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Stdio,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use unityrs_testkit::{
    child::{self, Fifo},
    guard::{self, DEADLOCK_GUARD},
};
use url_run_support::Origin;

#[test]
fn malformed_resolver_and_unreadable_roots_are_typed_startup_failures() {
    for (name, value) in [
        ("UNITYRS_TEST_RESOLVE", "malformed hook"),
        ("UNITYRS_TEST_EXTRA_ROOTS", "missing-root-file"),
    ] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let path = job(&dir, |_| {});
            let started = start(scene, &path, &[(name, value)], Stdio::null());
            let (code, line, err) = ended(scene, started);
            assert_eq!(code, 69, "{line} {err}");
            assert_eq!(line["reason"], "startup_failed");
            assert_eq!(line["detail"], name);
            assert!(!dir.join("spool/.unityrs-spool").exists());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn hostname_https_uses_only_the_hook_resolver_and_extra_root() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let root = dir.join("root.pem");
        // Existing synthetic localhost certificate, encoded as PEM because
        // the binary's hook accepts a PEM bundle of additional roots.
        fs::write(&root, pem(include_bytes!("../../unityrs-batch/tests/http_support/root.der")))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let (send, request) = mpsc::channel();
        std::thread::spawn(move || {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let mut config = ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions().unwrap().with_no_client_auth()
                .with_single_cert(vec![CertificateDer::from(include_bytes!("../../unityrs-batch/tests/http_support/leaf.der").to_vec())],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(include_bytes!("../../unityrs-batch/tests/http_support/key.der").to_vec()))).unwrap();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            let (socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
            socket.set_write_timeout(Some(DEADLOCK_GUARD)).unwrap();
            let mut tls = StreamOwned::new(ServerConnection::new(Arc::new(config)).unwrap(), socket);
            let mut head = Vec::new();
            let mut bytes = [0; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tls.read(&mut bytes).unwrap();
                assert!(n > 0);
                head.extend_from_slice(&bytes[..n]);
            }
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nnot a bundle").unwrap();
            tls.flush().unwrap();
            let _ = send.send((head, tls.conn.server_name().map(str::to_owned), tls.conn.alpn_protocol().map(<[u8]>::to_vec)));
        });
        let path = job(&dir, |spec| spec["inputs"] = json!([{"id":"a", "url":format!("https://localhost:{}/bundle", address.port())}]));
        let resolve = format!("localhost={address}");
        let s = start(scene, &path, &[("UNITYRS_TEST_RESOLVE", &resolve), ("UNITYRS_TEST_EXTRA_ROOTS", root.to_str().unwrap())], Stdio::null());
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 4, "{line} {err}");
        let (request, name, alpn) = guard::recv(&request, "TLS request");
        assert!(String::from_utf8(request).unwrap().starts_with("GET /bundle HTTP/1.1\r\n"));
        assert_eq!(name.as_deref(), Some("localhost"));
        assert_eq!(alpn.as_deref(), Some(b"http/1.1".as_slice()));
        let rows = rows(&dir);
        assert_eq!(rows[0]["status"], "staged");
        Ok(())
    }).unwrap();
}

#[test]
fn held_lookups_quarantine_every_permit_then_release_the_fast_host_and_local_copy() {
    for concurrency in [1, 2] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let origin = Origin::start();
            origin.body("/fast", b"not a bundle".to_vec());
            let trace = dir.join("trace");
            let release = dir.join("resolve-go");
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            // A bound listener, then closed, guarantees a refusing address
            // without a name lookup or a port another test may have reserved.
            let refused = TcpListener::bind("127.0.0.2:0")?;
            let port = refused.local_addr()?.port();
            drop(refused);
            let path = job(&dir, |spec| {
                let mut inputs: Vec<Value> = (0..concurrency).map(|n| json!({"id":format!("slow{n}"), "url":format!("http://slow.invalid:{port}/slow")})).collect();
                inputs.push(json!({"id":"fast", "url":format!("http://fast.invalid:{}/fast", origin.address.port())}));
                inputs.push(json!({"id":"local", "path":dir.join("a.bin")}));
                spec["inputs"] = json!(inputs);
                spec["download_concurrency"] = json!(concurrency);
                spec["http"] = json!({"attempts":2,"connect_timeout_ms":300,"backoff_initial_ms":0,"backoff_max_ms":0});
            });
            let resolve = format!("slow.invalid=127.0.0.2:{port}+hold:{},fast.invalid={}", release.display(), origin.address);
            let s = start(scene, &path, &[("UNITYRS_TEST_TRACE", trace.to_str().unwrap()), ("UNITYRS_TEST_RESOLVE", &resolve)], Stdio::null());
            let until = Instant::now() + DEADLOCK_GUARD;
            loop {
                let log = fs::read_to_string(&trace).unwrap_or_default();
                if log.lines().filter(|l| *l == "resolve slow.invalid held").count() == concurrency { break; }
                assert!(scene.running(s.live)?);
                assert!(Instant::now() < until, "all resolver slots were not held: {log}");
                std::thread::sleep(Duration::from_millis(1));
            }
            // Hold past all tries' connect windows, as X9 specifies. Lookup
            // closures retain their tokens even after the first try expires.
            let held_until = Instant::now() + Duration::from_millis(900);
            while Instant::now() < held_until {
                assert!(scene.running(s.live)?);
                assert_eq!(origin.requests("/fast"), 0);
                std::thread::sleep(Duration::from_millis(5));
            }
            fs::write(release, b"")?;
            let (code, line, err) = ended(scene, s);
            assert_eq!(code, 4, "{line} {err}");
            assert_eq!(origin.requests("/fast"), 1);
            let rows = rows(&dir);
            for n in 0..concurrency {
                let id = format!("slow{n}");
                let row = rows.iter().find(|r| r["id"] == id).unwrap();
                assert_eq!(row["reason"], "download_failed");
                assert_eq!(row["detail"], "connect");
            }
            for id in ["fast", "local"] { assert_eq!(rows.iter().find(|r| r["id"] == id).unwrap()["status"], "staged"); }
            assert_pool_trace(&fs::read_to_string(&trace)?, concurrency);
            Ok(())
        }).unwrap();
    }
}

#[test]
fn a_header_secret_in_a_panicking_stage_id_is_masked_on_stderr() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let origin = Origin::start();
        let trace = dir.join("trace");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let secret = "private-header-value".repeat(51) + "tail";
        assert_eq!(secret.len(), 1024);
        let path = job(&dir, |spec| {
            spec["inputs"] = json!([{"id":secret, "path":dir.join("a.bin")}]);
            spec["dependencies"] = json!([{"id":"unused", "url":origin.url("/unused"), "headers":{"X-Secret":{"env":"UNITYRS_TEST_HEADER"}}}]);
        });
        let point = format!("stage:{secret}=panic");
        let s = start(scene, &path, &[("UNITYRS_TEST_TRACE", trace.to_str().unwrap()), ("UNITYRS_TEST_POINT", &point), ("UNITYRS_TEST_HEADER", &secret)], Stdio::null());
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 70, "{line} {err}");
        assert!(!err.contains("private-header-value"), "{err}");
        assert!(err.contains("***"), "{err}");
        Ok(())
    }).unwrap();
}

#[test]
fn the_prestarted_stdin_watch_masks_a_literal_header_value() {
    use std::{net::UdpSocket, os::fd::OwnedFd};
    child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let origin = Origin::start();
            let trace = dir.join("trace");
            let go = dir.join("main-go");
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            let closed = UdpSocket::bind("127.0.0.2:0")?;
            let address = closed.local_addr()?;
            drop(closed);
            let socket = UdpSocket::bind("127.0.0.1:0")?;
            socket.connect(address)?;
            let path = job(&dir, |spec| {
                spec["stop_on_stdin_eof"] = json!(true);
                spec["dependencies"] = json!([{"id":"unused", "url":origin.url("/unused"), "headers":{"X-Secret":"stdin"}}]);
            });
            let point = format!("main=wait:{}", go.display());
            let s = start(scene, &path, &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ], Stdio::from(OwnedFd::from(socket.try_clone()?)));
            traced(scene, s.live, &trace, "main reached");
            socket.send(b"x")?;
            let until = Instant::now() + DEADLOCK_GUARD;
            let mut warning = Vec::new();
            loop {
                while let Ok(more) = s.err.0.try_recv() { warning.extend(more); }
                if warning.contains(&b'\n') { break; }
                assert!(scene.running(s.live)?);
                assert!(Instant::now() < until, "stdin warning never arrived");
                std::thread::sleep(Duration::from_millis(1));
            }
            let warning = String::from_utf8(warning).unwrap();
            assert_eq!(warning, "*** watch ended\n");
            fs::write(go, b"")?;
            let (code, line, err) = ended(scene, s);
            assert_eq!(code, 4, "{line} {err}");
            assert!(!err.contains("stdin"), "{err}");
            Ok(())
        }).unwrap();
}

fn rows(dir: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(dir.join("sink/_unityrs/r/1/bundles.0.ndjson"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn assert_pool_trace(log: &str, concurrency: usize) {
    let mut outstanding = [0_usize; 2];
    let mut held = 0;
    let mut local = false;
    let mut lookups = 0;
    for line in log.lines() {
        if line == "resolve slow.invalid held" {
            held += 1;
        }
        let fields: Vec<_> = line.splitn(5, ' ').collect();
        if fields.first() != Some(&"pool") {
            continue;
        }
        assert_eq!(fields.len(), 5, "{line}");
        let index = match fields[1] {
            "controller" => 0,
            "finisher" => 1,
            _ => panic!("unknown runtime: {line}"),
        };
        let count: usize = fields[3].parse().unwrap();
        match fields[2] {
            "spawn" => {
                outstanding[index] += 1;
                assert!(count <= concurrency + 1, "pool ceiling: {line}");
                if fields[4] == "a local copy" {
                    assert!(
                        held >= concurrency,
                        "local copy preceded held lookups: {log}"
                    );
                    local = true;
                }
                lookups += usize::from(fields[4] == "lookup");
            }
            "end" => outstanding[index] -= 1,
            _ => panic!("unknown pool event: {line}"),
        }
        assert_eq!(count, outstanding[index], "{line}");
    }
    assert!(local, "no local-copy task trace: {log}");
    assert!(lookups > concurrency, "missing lookup spawns: {log}");
    assert_eq!(outstanding, [0, 0], "pool tasks not home: {log}");
}
fn pem(bytes: &[u8]) -> String {
    const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for (n, chunk) in bytes.chunks(3).enumerate() {
        let word = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i > chunk.len() {
                '='
            } else {
                char::from(BASE64[((word >> shift) & 63) as usize])
            });
        }
        if n % 16 == 15 {
            out.push('\n');
        }
    }
    out.push_str("\n-----END CERTIFICATE-----\n");
    out
}
