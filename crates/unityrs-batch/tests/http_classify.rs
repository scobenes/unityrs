//! Origin-form request preparation and fixed diagnostics.
#[test]
fn preparation_sends_origin_form_and_bundle_defaults_are_replaceable() {
    use hyper::http::{HeaderMap, HeaderValue, Method};
    use unityrs_batch::http::{Payload, prepare};
    let url = url::Url::parse("https://localhost:443/a?secret=one#fragment").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_static("custom"));
    headers.insert("accept", HeaderValue::from_static("application/test"));
    let request = prepare(&url, Method::GET, headers, Payload::default()).unwrap();
    assert_eq!(request.uri().to_string(), "/a?secret=one");
    assert_eq!(request.headers()["host"], "localhost");
    assert_eq!(request.headers()["accept-encoding"], "identity");
    assert_eq!(request.headers()["user-agent"], "custom");
    assert_eq!(request.headers()["accept"], "application/test");
    let request = prepare(&url, Method::GET, HeaderMap::new(), Payload::default()).unwrap();
    assert_eq!(
        request.headers()["user-agent"],
        concat!("unityrs/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(request.headers()["accept"], "*/*");
}

#[test]
fn invalid_urls_and_reserved_headers_return_fixed_errors() {
    use hyper::http::{HeaderMap, HeaderValue, Method};
    use unityrs_batch::http::{Error, Payload, prepare};
    for address in [
        "https://user:private@localhost/",
        "ftp://localhost/input",
        "https://localhost/",
    ] {
        let url = url::Url::parse(address).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("secret"));
        assert_eq!(
            prepare(&url, Method::GET, headers, Payload::default()).err(),
            Some(Error::InvalidRequest)
        );
    }
}

#[test]
fn prepared_values_refuse_over_the_exact_16_kib_value_cap() {
    use hyper::http::{HeaderMap, HeaderValue, Method};
    use unityrs_batch::http::{Error, Payload, prepare};
    let url = url::Url::parse("https://localhost/").unwrap();
    for size in [16_384, 16_385] {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-value",
            HeaderValue::from_bytes(&vec![b'a'; size]).unwrap(),
        );
        let result = prepare(&url, Method::GET, headers, Payload::default());
        if size == 16_384 {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.err(), Some(Error::InvalidRequest));
        }
    }
}
