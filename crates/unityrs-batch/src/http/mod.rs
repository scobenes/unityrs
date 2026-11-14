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
pub mod shared;
pub mod socket;
mod tls;

#[cfg(test)]
mod driver_tests;
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
        self.request_inner(permit, url, request, timeout, None, None)
            .await
    }
    /// Staging acquires the lookup token itself, so it can check its canonical
    /// cutoff after acquisition and start active body time before resolution.
    pub async fn request_with_token(
        &self,
        permit: &mut permit::RequestPermit,
        url: &Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: permit::OperationToken,
    ) -> Result<Response<Stream>, Error> {
        if !token.belongs_to(permit) {
            return Err(Error::InvalidRequest);
        }
        self.request_inner(permit, url, request, timeout, Some(token), None)
            .await
    }
    async fn request_inner(
        &self,
        permit: &mut permit::RequestPermit,
        url: &Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: Option<permit::OperationToken>,
        connected: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Response<Stream>, Error> {
        request::validate_url(url)?;
        request::validate_request(&request, Some(url))?;
        // Lookup owns its independent charge until the closure returns. The
        // TLS envelope grows by that amount only after lookup has quiesced.
        let mut envelope = permit
            .charge(LIBRARY_ENVELOPE - resolve::LOOKUP_ENVELOPE)
            .ok_or(Error::Memory)?;
        let io = self
            .connector
            .connect(permit, url, timeout, token, &mut envelope)
            .await?;
        if let Some(connected) = connected {
            connected.store(true, std::sync::atomic::Ordering::Release);
        }
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
