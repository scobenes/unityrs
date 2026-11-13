//! A1's minimal wire entries force the largest rustls vector capacities.
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::sync::Arc;

pub fn config(scene: &str) -> ServerConfig {
    let tls13 = matches!(scene, "tls13-minimal" | "tls13-authorities");
    let width = match scene {
        "tls12-empty" | "tls13-minimal" => Some(0),
        "tls12-one-byte" | "combined-minimal" | "combined-success" | "maximal-combined" => Some(1),
        _ => None,
    };
    let authorities = matches!(
        scene,
        "authorities-cap32768"
            | "combined-minimal"
            | "combined-success"
            | "maximal-combined"
            | "tls13-authorities"
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[if tls13 {
            &rustls::version::TLS13
        } else {
            &rustls::version::TLS12
        }])
        .unwrap();
    let builder = if authorities {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(super::tls::ROOT.to_vec()))
            .unwrap();
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .clear_root_hint_subjects()
                .add_root_hint_subjects(
                    std::iter::repeat_with(|| rustls::DistinguishedName::from(vec![0])).take(
                        if matches!(scene, "combined-success" | "maximal-combined") {
                            18_000
                        } else if tls13 {
                            20_000
                        } else {
                            21_700
                        },
                    ),
                )
                .allow_unauthenticated()
                .build()
                .unwrap();
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let leaf = include_bytes!("leaf.der");
    let mut chain = vec![CertificateDer::from(leaf.to_vec())];
    if let Some(width) = width {
        let overhead = if tls13 { 5 } else { 3 };
        // Include the real leaf, list framing and handshake framing under
        // 64 KiB, while crossing each vector's last capacity doubling.
        let count = if matches!(scene, "combined-success" | "maximal-combined") {
            2_000
        } else {
            ((if tls13 { 60_000 } else { 65_400 }) - leaf.len() - overhead) / (width + overhead)
        };
        chain.extend(std::iter::repeat_with(|| CertificateDer::from(vec![0; width])).take(count));
    }
    let mut config = builder
        .with_single_cert_with_ocsp(
            chain,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(include_bytes!("key.der").to_vec())),
            if matches!(scene, "tls12-ocsp" | "combined-minimal") {
                vec![0; 65_400]
            } else {
                Vec::new()
            },
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.max_fragment_size = Some(12_288);
    config
}
