//! One immutable startup configuration. No environment or host trust store.
use super::Error;
use rustls::{ClientConfig, RootCertStore, client::Resumption};
use std::sync::Arc;

pub(crate) fn configuration() -> Result<Arc<ClientConfig>, Error> {
    let mut roots = RootCertStore::empty();
    roots
        .roots
        .try_reserve_exact(webpki_roots::TLS_SERVER_ROOTS.len())
        .map_err(|_| Error::Memory)?;
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    build(roots)
}
fn build(roots: RootCertStore) -> Result<Arc<ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| Error::InvalidRequest)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config
        .alpn_protocols
        .try_reserve_exact(1)
        .map_err(|_| Error::Memory)?;
    let mut alpn = Vec::new();
    alpn.try_reserve_exact(8).map_err(|_| Error::Memory)?;
    alpn.extend_from_slice(b"http/1.1");
    config.alpn_protocols.push(alpn);
    config.resumption = Resumption::disabled();
    config.enable_early_data = false;
    config.cert_decompressors.clear();
    Ok(Arc::new(config))
}

/// Test roots are deliberately absent from the production API and binary.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn test_configuration(
    extra_roots: &[rustls::pki_types::CertificateDer<'static>],
) -> Result<Arc<ClientConfig>, Error> {
    let mut roots = RootCertStore::empty();
    roots
        .roots
        .try_reserve_exact(webpki_roots::TLS_SERVER_ROOTS.len() + extra_roots.len())
        .map_err(|_| Error::Memory)?;
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for root in extra_roots {
        roots.add(root.clone()).map_err(|_| Error::InvalidRequest)?;
    }
    build(roots)
}
