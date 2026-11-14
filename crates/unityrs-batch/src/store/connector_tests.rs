use super::*;
use hyper::http::HeaderValue;
use object_store::PutPayload;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn a_uri_passes_only_as_the_bytes_the_url_crate_sends() {
    for text in [
        "https://h.test/b/k%20x.png",
        "https://h.test:9000/b/%25%23%7E?list-type=2&prefix=a%2Fb&max-keys=1000",
        "http://[::1]:9000/b/k",
        "https://h.test/b/k?uploadId=x-1&partNumber=2",
        "https://h.test/b/a~b!$&'()*+,;=:@",
    ] {
        let uri: Uri = text.parse().unwrap();
        let url = url_of(&uri).unwrap();
        assert_eq!(
            &url[url::Position::BeforePath..url::Position::AfterQuery],
            uri.path_and_query().unwrap().as_str()
        );
    }
    for text in [
        // The `url` crate would percent-encode these bytes of a query.
        "https://h.test/b?prefix='x",
        // A dot segment it would resolve.
        "https://h.test/b/./k",
        "https://h.test/b/%2e%2E/k",
        // No scheme or authority.
        "/b/k",
    ] {
        let uri: Uri = text.parse().unwrap();
        assert_eq!(url_of(&uri).unwrap_err(), Fault::Internal, "{text}");
    }
}

#[test]
fn an_origin_admits_its_scheme_host_and_port_alone() {
    let origin = Origin::of(&url("https://bucket.s3.test/")).unwrap();
    assert!(origin.admits(&url("https://bucket.s3.test:443/k")));
    for other in [
        "http://bucket.s3.test/k",
        "https://bucket.s3.test:8443/k",
        "https://other.s3.test/k",
    ] {
        assert!(!origin.admits(&url(other)), "{other}");
    }
    let plain = Origin::of(&url("http://127.0.0.1:9000/b")).unwrap();
    assert!(plain.admits(&url("http://127.0.0.1:9000/b/k")));
    assert!(!plain.admits(&url("https://127.0.0.1:9000/b/k")));
}

#[test]
fn host_and_length_are_checked_and_left_to_the_service() {
    let at = url("https://h.test:9000/b/k");
    let mut map = HeaderMap::new();
    map.insert(HOST, HeaderValue::from_static("h.test:9000"));
    map.insert(CONTENT_LENGTH, HeaderValue::from_static("3"));
    map.insert("x-amz-date", HeaderValue::from_static("20261113T000000Z"));
    let out = headers(map.clone(), &at, 3).unwrap();
    assert!(!out.contains_key(HOST) && !out.contains_key(CONTENT_LENGTH));
    assert!(out.contains_key("x-amz-date"));
    assert_eq!(headers(map.clone(), &at, 4).unwrap_err(), Fault::Internal);
    for (name, value) in [
        ("content-length", "+3"),
        ("content-length", ""),
        ("host", "h.test"),
    ] {
        let mut bad = map.clone();
        bad.insert(name, HeaderValue::from_static(value));
        assert_eq!(
            headers(bad, &at, 3).unwrap_err(),
            Fault::Internal,
            "{name}: {value}"
        );
    }
    assert!(headers(HeaderMap::new(), &at, 0).is_ok());
}

#[test]
fn decimal_reads_digits_alone() {
    assert_eq!(decimal(b"0"), Some(0));
    assert_eq!(decimal(b"1099511627776"), Some(1 << 40));
    assert_eq!(
        decimal(b"9999999999999999999"),
        Some(9_999_999_999_999_999_999)
    );
    for bad in [
        &b""[..],
        b"-1",
        b" 1",
        b"1 ",
        b"0x10",
        b"99999999999999999999",
    ] {
        assert_eq!(decimal(bad), None);
    }
}

#[test]
fn a_payload_is_one_uncopied_bytes() {
    let bytes = Bytes::from(vec![7_u8; 1_024]);
    let body = HttpRequestBody::from(PutPayload::from(bytes.clone()));
    let mut one = payload(body).unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut one).poll_frame(&mut cx) else {
        panic!("one frame");
    };
    // The same storage, not a copy.
    assert_eq!(frame.into_data().unwrap().as_ptr(), bytes.as_ptr());
    assert!(payload(HttpRequestBody::from(PutPayload::new())).is_ok());
    assert!(payload(HttpRequestBody::from(Bytes::from_static(b"<x/>"))).is_ok());
    let two: PutPayload = vec![Bytes::from_static(b"a"), Bytes::from_static(b"b")]
        .into_iter()
        .collect();
    assert_eq!(
        payload(HttpRequestBody::from(two)).err(),
        Some(Fault::Internal)
    );
}
