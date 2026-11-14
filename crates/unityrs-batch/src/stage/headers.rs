//! Active header preparation. Environment values are preflight-resolved by the
//! caller and shared in `C_hdr`; this layer never reads the process environment.
use crate::{
    http::{Error, Payload},
    spec::model::{BundleRef, HeaderValue, Spec},
};
use hyper::{
    Request,
    http::{HeaderMap, HeaderName, HeaderValue as Value, Method},
};
use url::Url;

pub trait Values {
    fn get(&self, name: &str) -> Option<&str>;
    /// The header name as phase 1 parsed it ([`Names`]); [`prepare`]
    /// parses any name this lacks.
    fn name(&self, _name: &str) -> Option<&HeaderName> {
        None
    }
}
/// Each distinct header name a spec's bundles send, parsed once in phase 1
/// (`PreparedRun`), by its spelling in the spec. A custom name's storage is
/// a `Bytes` that its first clone moves to shared, counted storage (an
/// allocation, and an infallible one): that clone is taken here, so each
/// request's clone only counts a reference.
#[derive(Default)]
pub struct Names(Box<[(Box<str>, HeaderName)]>);
impl Names {
    pub(crate) fn from_spec(spec: &Spec) -> Self {
        let mut names: Vec<(Box<str>, HeaderName)> = Vec::new();
        for bundle in spec.bundles() {
            for (name, _) in bundle.headers() {
                let Err(at) = names.binary_search_by(|(n, _)| (**n).cmp(name)) else {
                    continue;
                };
                // An invalid name is left to `prepare`, which refuses it.
                let Ok(parsed) = HeaderName::from_bytes(name.as_bytes()) else {
                    continue;
                };
                let shared = parsed.clone();
                drop(parsed);
                names.insert(at, (name.into(), shared));
            }
        }
        Self(names.into_boxed_slice())
    }
    fn get(&self, name: &str) -> Option<&HeaderName> {
        self.0
            .binary_search_by(|(n, _)| (**n).cmp(name))
            .ok()
            .and_then(|at| self.0.get(at))
            .map(|(_, parsed)| parsed)
    }
}
/// A run's header inputs: preflight's values and phase 1's names.
pub(crate) struct Prepared<'a, V> {
    pub values: &'a V,
    pub names: &'a Names,
}
impl<V: Values> Values for Prepared<'_, V> {
    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name)
    }
    fn name(&self, name: &str) -> Option<&HeaderName> {
        self.names.get(name)
    }
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
        let name = match values.name(name) {
            Some(parsed) => parsed.clone(),
            None => HeaderName::from_bytes(name.as_bytes()).map_err(|_| Error::InvalidRequest)?,
        };
        headers.insert(
            name,
            Value::from_maybe_shared(hyper::body::Bytes::from(bytes))
                .map_err(|_| Error::InvalidRequest)?,
        );
    }
    crate::http::prepare(url, Method::GET, headers, Payload::default())
}
