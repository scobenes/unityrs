//! A whole library run staging over `https` through the run's test hooks:
//! the resolver in place of the system's and the synthetic root as an extra
//! trust anchor (`batch.md` 2.11), the engine's own service otherwise.
#![cfg(feature = "test-hooks")]
mod common;
mod http_support;
mod publication_support;
mod run_support;
use http_support::tls::{FixedResolver, Peer, ROOT};
use rustls::pki_types::CertificateDer;
use std::{fs, sync::Arc};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    run::{self, Hooks, Network, Options},
};
use unityrs_testkit::scratch::Scratch;
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
#[test]
fn an_https_url_stages_through_the_run_hooks_and_closes_with_a_reset() {
    let _serial = http_support::tls::lock();
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "textures-five")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("url-tls-run");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, expected) = run_support::instantiate(&case, &scene, &scratch);
    let input = &mut spec["inputs"][0];
    let bytes = fs::read(input["path"].as_str().unwrap()).unwrap();
    let mut response =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", bytes.len()).into_bytes();
    response.extend_from_slice(&bytes);
    let peer = Peer::tls(response, true);
    let fields = input.as_object_mut().unwrap();
    fields.remove("path");
    fields.insert(
        "url".into(),
        format!("https://localhost:{}/b0", peer.address.port()).into(),
    );
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let options = Options {
        network: Some(Network {
            hooks: Some(Hooks {
                resolver: Arc::new(FixedResolver(vec![peer.address])),
                extra_roots: vec![CertificateDer::from(ROOT.to_vec())],
            }),
            ..Network::system()
        }),
        ..Options::default()
    };
    let end = run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &options,
    )
    .unwrap();
    assert_eq!(end.code.value(), 0);
    let seen = peer.finish().unwrap();
    assert_eq!(seen.server_name.as_deref(), Some("localhost"));
    assert!(seen.request.starts_with(b"GET /b0 HTTP/1.1\r\n"));
    // The finished request's connection is reset, never closed politely.
    assert!(seen.reset && !seen.close_notify, "{seen:?}");
    run_support::check(&scratch, &spec, &case, expected, 123);
}
