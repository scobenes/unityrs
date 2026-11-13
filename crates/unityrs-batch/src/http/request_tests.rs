use super::{Error, Payload, request::validate_request};
use hyper::{Request, http::HeaderValue};

fn prepared() -> Request<Payload> {
    super::prepare(
        &url::Url::parse("https://localhost:8443/path").unwrap(),
        hyper::http::Method::GET,
        hyper::http::HeaderMap::new(),
        Payload::default(),
    )
    .unwrap()
}

#[test]
fn boundary_validation_refuses_reserved_overrides_and_invalid_host() {
    for name in [
        "content-length",
        "transfer-encoding",
        "connection",
        "range",
        "accept-encoding",
    ] {
        let mut request = prepared();
        request
            .headers_mut()
            .insert(name, HeaderValue::from_static("unexpected"));
        assert_eq!(
            validate_request(&request, None),
            Err(Error::InvalidRequest),
            "{name}"
        );
    }
    let mut missing = prepared();
    missing.headers_mut().remove("host");
    assert_eq!(validate_request(&missing, None), Err(Error::InvalidRequest));
    let mut duplicate = prepared();
    duplicate
        .headers_mut()
        .append("host", HeaderValue::from_static("localhost:8443"));
    assert_eq!(
        validate_request(&duplicate, None),
        Err(Error::InvalidRequest)
    );
}

#[test]
fn boundary_validation_requires_the_urls_exact_authority() {
    let url = url::Url::parse("https://localhost:8443/path").unwrap();
    let mut request = prepared();
    assert_eq!(validate_request(&request, Some(&url)), Ok(()));
    request
        .headers_mut()
        .insert("host", HeaderValue::from_static("other.test:8443"));
    assert_eq!(
        validate_request(&request, Some(&url)),
        Err(Error::InvalidRequest)
    );
}
