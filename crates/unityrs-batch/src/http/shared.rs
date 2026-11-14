//! A request that reports when its connection is up, which the first row of
//! `BATCH_SPEC.md` 1.6's table reads: a try that failed before then sent
//! nothing. Its lookup token is the try's, taken before its timer started.
//! Staging's backoff and `Retry-After` (`stage/retry.rs`) are the store's
//! too. Nothing here renders a header or a URL.
use super::{
    Error, Payload, Response, Stream,
    permit::{OperationToken, RequestPermit},
};
use std::{sync::atomic::AtomicBool, time::Duration};
use url::Url;

impl super::Service {
    /// [`Service::request`](super::Service::request), setting `connected`
    /// once the connection is up: its resolve, connects and, for `https`,
    /// handshake done, before the request is written.
    pub async fn request_observed(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        request: hyper::Request<Payload>,
        timeout: Duration,
        token: OperationToken,
        connected: &AtomicBool,
    ) -> Result<Response<Stream>, Error> {
        if !token.belongs_to(permit) {
            return Err(Error::InvalidRequest);
        }
        self.request_inner(permit, url, request, timeout, Some(token), Some(connected))
            .await
    }
}
