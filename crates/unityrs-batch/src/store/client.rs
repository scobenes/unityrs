//! The one place a store client is built (a source scan,
//! `tests/store_scan.rs`, holds every `AmazonS3Builder`, `from_env` and
//! `reqwest` to this file). Each client starts from
//! `AmazonS3Builder::new()`, never `from_env`, which reads every `AWS_*`
//! setting (E37); takes the sink's credentials through `with_credentials`,
//! which skips the fallback chain; retries nothing (`max_retries` 0), as the
//! engine makes every try itself; fences with `S3ConditionalPut::ETagMatch`;
//! and always sends through the engine's [`Connector`]. `object_store` is
//! built without its `reqwest` client (which reads proxy variables), and the
//! scan keeps it out should a feature bring it back. No
//! `allow_http`: the connector admits only the bucket endpoint's origin.
use super::{
    attempt::Fault,
    connector::{Connector, Origin, Transport},
};
use object_store::{
    BackoffConfig, ClientOptions, RetryConfig,
    aws::{AmazonS3, AmazonS3Builder, AwsCredentialProvider, S3ConditionalPut},
};
use std::time::Duration;
use url::{Host, Url};

/// A sink's addressing (`docs/BATCH_SPEC.md` 1.6), as validated.
#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    pub bucket: &'a str,
    pub region: &'a str,
    /// `None`: AWS, in the `amazonaws.com` partition.
    pub endpoint: Option<&'a Url>,
    pub path_style: bool,
}

impl Settings<'_> {
    /// A host name lowercases a bucket with an uppercase letter or `_`, and
    /// `<bucket>.<address>` is not a host: validation gives both path style
    /// (1.1), and the client refuses them otherwise.
    fn checked(&self) -> Result<(), Fault> {
        let ip = self
            .endpoint
            .is_some_and(|url| matches!(url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_))));
        let named = self
            .bucket
            .bytes()
            .all(|b| !b.is_ascii_uppercase() && b != b'_');
        if self.path_style || (!ip && named) {
            Ok(())
        } else {
            Err(Fault::Internal)
        }
    }
}

/// The endpoint the engine passes `object_store`: path style, the spec's,
/// to which it adds `/<bucket>`; virtual-hosted, `<scheme>://<bucket>.<host>
/// [:<port>]`, built here, since `object_store` uses a custom endpoint
/// unchanged in that mode. `None` for AWS.
fn endpoint(settings: &Settings<'_>) -> Result<Option<String>, Fault> {
    let Some(url) = settings.endpoint else {
        return Ok(None);
    };
    let mut text = String::new();
    if settings.path_style {
        let written = url.as_str();
        text.try_reserve_exact(written.len())
            .map_err(|_| Fault::Memory)?;
        text.push_str(written);
    } else {
        let host = url.host_str().ok_or(Fault::Internal)?;
        let port = &url[url::Position::AfterHost..url::Position::AfterPort];
        text.try_reserve_exact(
            url.scheme().len() + 4 + settings.bucket.len() + host.len() + port.len(),
        )
        .map_err(|_| Fault::Memory)?;
        text.push_str(url.scheme());
        text.push_str("://");
        text.push_str(settings.bucket);
        text.push('.');
        text.push_str(host);
        text.push_str(port);
    }
    Ok(Some(text))
}

/// The URL every request of the sink starts with: the bucket's, in all four
/// cells (path or virtual-hosted style, an endpoint or AWS).
pub fn bucket_endpoint(settings: &Settings<'_>) -> Result<Url, Fault> {
    settings.checked()?;
    let text = match endpoint(settings)? {
        Some(endpoint) if settings.path_style => {
            format!("{}/{}", endpoint.trim_end_matches('/'), settings.bucket)
        }
        Some(endpoint) => endpoint,
        None if settings.path_style => {
            format!(
                "https://s3.{}.amazonaws.com/{}",
                settings.region, settings.bucket
            )
        }
        None => format!(
            "https://{}.s3.{}.amazonaws.com",
            settings.bucket, settings.region
        ),
    };
    Url::parse(&text).map_err(|_| Fault::Internal)
}

/// The origin a sink's connector admits.
pub fn origin(settings: &Settings<'_>) -> Result<Origin, Fault> {
    Origin::of(&bucket_endpoint(settings)?)
}

/// A client of the sink. `content_type` is a multipart client's default
/// (E37): `create_multipart` takes no options, so the controller has one
/// client per multipart Content-Type; a single PUT sets its own.
pub fn build<T: Transport>(
    settings: &Settings<'_>,
    content_type: Option<&str>,
    credentials: &AwsCredentialProvider,
    connector: &Connector<T>,
) -> Result<AmazonS3, Fault> {
    settings.checked()?;
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(settings.bucket)
        .with_region(settings.region)
        .with_credentials(credentials.clone())
        .with_virtual_hosted_style_request(!settings.path_style)
        .with_conditional_put(S3ConditionalPut::ETagMatch)
        .with_retry(RetryConfig {
            backoff: BackoffConfig::default(),
            max_retries: 0,
            retry_timeout: Duration::ZERO,
        })
        .with_http_connector(connector.clone());
    if let Some(endpoint) = endpoint(settings)? {
        builder = builder.with_endpoint(endpoint);
    }
    if let Some(content_type) = content_type {
        builder = builder
            .with_client_options(ClientOptions::new().with_default_content_type(content_type));
    }
    builder.build().map_err(|_| Fault::Internal)
}
