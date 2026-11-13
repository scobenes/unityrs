//! Bounded one-request HTTP/1.1 transport. URL execution remains refused.
//! Redirects, retries, body timers and spool writes belong to staging.
mod addresses;
mod classify;
mod connect;
mod error;
mod io;
pub mod permit;
pub mod request;
pub mod resolve;
mod response;
pub mod socket;
mod tls;

#[cfg(test)]
mod request_tests;

use std::{sync::Arc, time::Duration};
use tokio::runtime::Handle;
use url::Url;

pub use error::{Detail, Error, Failure, RetryClass};
pub use io::Stream;
pub use request::{Payload, prepare};
pub use response::{Data, HEAD_CAP, LIBRARY_ENVELOPE, Response, exchange};

/// One immutable TLS configuration shared by every connection. The caller's
/// prewarmed blocking pool and bounded socket table outlive all requests.
pub struct Service {
    connector: connect::Connector,
}
impl Service {
    /// Startup only; its fixed allocations are in `C_base`.
    pub fn new(pool: Handle, table: Arc<dyn socket::SocketTable>) -> Result<Self, Error> {
        Ok(Self {
            connector: connect::Connector {
                pool,
                table,
                tls: tls::configuration()?,
                resolver: resolve::Source::System,
            },
        })
    }
    /// The caller prepares an origin-form request from this same URL. Waiting
    /// for a previous lookup token precedes the connection timer. No runtime or
    /// connection task is spawned here; the response owns the complete driver.
    /// The polling runtime must have both IO and time drivers enabled.
    pub async fn request(
        &self,
        permit: &mut permit::RequestPermit,
        url: &Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
    ) -> Result<Response<Stream>, Error> {
        request::validate_url(url)?;
        request::validate_request(&request, Some(url))?;
        let envelope = permit.charge(LIBRARY_ENVELOPE).ok_or(Error::Memory)?;
        let io = self.connector.connect(permit, url, timeout).await?;
        response::exchange_charged(io, envelope, request).await
    }
    /// Only the test build can replace DNS or add trust anchors. This consumes
    /// the service before it can be shared; no live configuration is mutated.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_test_hooks(
        mut self,
        resolver: Arc<dyn resolve::Resolver>,
        extra_roots: &[rustls::pki_types::CertificateDer<'static>],
    ) -> Result<Self, Error> {
        self.connector.resolver = resolve::Source::Hook(resolver);
        self.connector.tls = tls::test_configuration(extra_roots)?;
        Ok(self)
    }
}
