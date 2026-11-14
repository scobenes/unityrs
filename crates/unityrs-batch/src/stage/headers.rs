//! Active header preparation. Environment values are preflight-resolved by the
//! caller and shared in `C_hdr`; this layer never reads the process environment.
use crate::{
    http::{Error, Payload},
    spec::model::{BundleRef, HeaderValue},
};
use hyper::{
    Request,
    http::{HeaderMap, HeaderName, HeaderValue as Value, Method},
};
use url::Url;

pub trait Values {
    fn get(&self, name: &str) -> Option<&str>;
}
impl Values for () {
    fn get(&self, _: &str) -> Option<&str> {
        None
    }
}
/// Only active requests own a map; no parsed maps are retained per bundle.
/// Map/value storage overlaps the transport's measured library envelope.
pub(crate) fn prepare(
    bundle: BundleRef<'_>,
    values: &impl Values,
    url: &Url,
) -> Result<Request<Payload>, Error> {
    let mut headers = HeaderMap::new();
    headers.try_reserve(36).map_err(|_| Error::Memory)?;
    for (name, value) in bundle.headers() {
        let text = match value {
            HeaderValue::Literal(text) => text,
            HeaderValue::Env(name) => values.get(name).ok_or(Error::InvalidRequest)?,
        };
        if text.len() > 16 * 1024 {
            return Err(Error::InvalidRequest);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(text.len())
            .map_err(|_| Error::Memory)?;
        bytes.extend_from_slice(text.as_bytes());
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| Error::InvalidRequest)?,
            Value::from_maybe_shared(hyper::body::Bytes::from(bytes))
                .map_err(|_| Error::InvalidRequest)?,
        );
    }
    crate::http::prepare(url, Method::GET, headers, Payload::default())
}
