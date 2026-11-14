//! The store client over an in-memory transport:
//! addressing in every mode, the engine's headers, keys byte for byte,
//! the connector's one-request and URI checks, and credentials that never
//! render.
mod store_support;
use hyper::{body::Bytes, http::Request};
use object_store::{
    ClientOptions, ObjectStoreExt, PutPayload,
    client::{HttpConnector, HttpRequestBody},
    multipart::MultipartStore,
    path::Path,
};
use std::{sync::Arc, time::Duration};
use store_support::{Harness, Reply, Script, credentials, endpoint, run};
use unityrs_batch::store::{
    Kind,
    attempt::{Fault, Tries, TrySlot},
    classify::{Action, Ended},
    client::{self, Settings},
    connector::Connector,
};
use url::Url;

fn ok() -> Reply {
    Reply::status(200, "ETag: \"d41d8cd98f00b204e9800998ecf8427e\"\r\n", b"")
}

fn put(h: &Harness, key: &str) -> Result<(), Fault> {
    let path = Path::parse(key).unwrap();
    let done = h.run(Kind::Put, |_| {
        h.store
            .put(&path, PutPayload::from(Bytes::from_static(b"png")))
    })?;
    assert_eq!(done.verdict.unwrap().action, Action::Success, "{key:?}");
    Ok(())
}

#[test]
fn requests_carry_the_engine_headers_and_a_signature() {
    let h = Harness::new([ok()]);
    put(&h, "a/b.png").unwrap();
    let seen = &h.script.seen;
    assert_eq!(seen.line(0), "PUT /bucket/a/b.png HTTP/1.1");
    assert_eq!(seen.header(0, "host").unwrap(), "s3.example.test:9000");
    assert_eq!(
        seen.header(0, "user-agent").unwrap(),
        concat!("unityrs/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(seen.header(0, "accept").unwrap(), "*/*");
    assert_eq!(seen.header(0, "accept-encoding").unwrap(), "identity");
    assert_eq!(seen.header(0, "content-length").unwrap(), "3");
    assert!(
        seen.header(0, "authorization")
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")
    );
    // The signed payload hash, not UNSIGNED-PAYLOAD (E37's transit integrity).
    assert_eq!(seen.header(0, "x-amz-content-sha256").unwrap().len(), 64);
    for absent in [
        "x-amz-checksum-sha256",
        "x-amz-security-token",
        "cache-control",
        "x-amz-acl",
    ] {
        assert!(seen.header(0, absent).is_none(), "{absent}");
    }
    assert!(seen.request(0).ends_with("\r\n\r\npng"));
}

#[test]
fn a_multipart_client_sends_its_default_content_type_and_a_token() {
    let url = endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let body = b"<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>a.zip</Key>\
        <UploadId>upload-1</UploadId></InitiateMultipartUploadResult>";
    let script = Script::new([Reply::status(200, "", body)]);
    let connector = Connector::new(
        script.clone(),
        client::origin(&settings).unwrap(),
        Duration::from_secs(1),
    );
    let creds = credentials("AKIDEXAMPLE", "secret", Some("token-value"));
    let store = client::build(&settings, Some("application/zip"), &creds, &connector).unwrap();
    let pool = unityrs_batch::http::permit::PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Create);
    let (stop, unsettled) = (
        std::sync::atomic::AtomicBool::new(false),
        std::sync::atomic::AtomicU64::new(0),
    );
    let tries = Tries {
        policy: store_support::policy(1),
        control: &stop,
        unsettled: &unsettled,
    };
    let path = Path::parse("a.zip").unwrap();
    let done = run(tries.run(&slot, Kind::Create, |_| {
        let (store, path) = (&store, &path);
        async move { store.create_multipart(path).await.map_err(|_| Ended::Error) }
    }))
    .unwrap();
    assert_eq!(done.value.unwrap(), "upload-1");
    assert_eq!(script.seen.line(0), "POST /bucket/a.zip?uploads= HTTP/1.1");
    assert_eq!(
        script.seen.header(0, "content-type").unwrap(),
        "application/zip"
    );
    assert_eq!(
        script.seen.header(0, "x-amz-security-token").unwrap(),
        "token-value"
    );
    assert_eq!(unsettled.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(slot.take_permit().is_some());
}

/// The request line's path and `Host` for `key` in each addressing cell.
fn addressed(
    bucket: &str,
    region: &str,
    endpoint: Option<&str>,
    path_style: bool,
) -> (String, String, Url) {
    let url = endpoint.map(|e| Url::parse(e).unwrap());
    let settings = Settings {
        bucket,
        region,
        endpoint: url.as_ref(),
        path_style,
    };
    let h = Harness::with(&settings, None, [ok()]);
    put(&h, "k/x.png").unwrap();
    let line = h.script.seen.line(0);
    let path = line.split(' ').nth(1).unwrap().to_owned();
    (
        path,
        h.script.seen.header(0, "host").unwrap(),
        client::bucket_endpoint(&settings).unwrap(),
    )
}

/// A bucket, an endpoint, the style, and the path, `Host` and base it gives.
type Cell = (
    &'static str,
    Option<&'static str>,
    bool,
    &'static str,
    &'static str,
    &'static str,
);

#[test]
fn every_addressing_cell_maps_to_its_bucket_endpoint() {
    let cells: [Cell; 9] = [
        (
            "bucket",
            Some("https://s3.example.test:9000"),
            true,
            "/bucket/k/x.png",
            "s3.example.test:9000",
            "https://s3.example.test:9000/bucket",
        ),
        (
            "bucket",
            Some("https://s3.example.test/"),
            true,
            "/bucket/k/x.png",
            "s3.example.test",
            "https://s3.example.test/bucket",
        ),
        (
            "bucket",
            Some("http://s3.example.test:8080"),
            true,
            "/bucket/k/x.png",
            "s3.example.test:8080",
            "http://s3.example.test:8080/bucket",
        ),
        (
            "bucket",
            Some("https://s3.example.test:9000"),
            false,
            "/k/x.png",
            "bucket.s3.example.test:9000",
            "https://bucket.s3.example.test:9000/",
        ),
        (
            "bucket",
            Some("http://s3.example.test/"),
            false,
            "/k/x.png",
            "bucket.s3.example.test",
            "http://bucket.s3.example.test/",
        ),
        (
            "bucket",
            None,
            true,
            "/bucket/k/x.png",
            "s3.eu-west-1.amazonaws.com",
            "https://s3.eu-west-1.amazonaws.com/bucket",
        ),
        (
            "bucket",
            None,
            false,
            "/k/x.png",
            "bucket.s3.eu-west-1.amazonaws.com",
            "https://bucket.s3.eu-west-1.amazonaws.com/",
        ),
        // An IP host and a bucket a host name would lowercase: path style.
        (
            "bucket",
            Some("http://127.0.0.1:9000"),
            true,
            "/bucket/k/x.png",
            "127.0.0.1:9000",
            "http://127.0.0.1:9000/bucket",
        ),
        (
            "My_Bucket",
            Some("http://[::1]:9000"),
            true,
            "/My_Bucket/k/x.png",
            "[::1]:9000",
            "http://[::1]:9000/My_Bucket",
        ),
    ];
    for (bucket, endpoint, path_style, path, host, base) in cells {
        let (seen_path, seen_host, bucket_endpoint) =
            addressed(bucket, "eu-west-1", endpoint, path_style);
        assert_eq!(
            (seen_path.as_str(), seen_host.as_str()),
            (path, host),
            "{endpoint:?} {path_style}"
        );
        assert_eq!(bucket_endpoint.as_str(), base);
    }
}

#[test]
fn virtual_hosted_style_refuses_an_ip_host_and_a_bucket_a_host_would_change() {
    let creds = credentials("AKIDEXAMPLE", "secret", None);
    for (bucket, endpoint) in [
        ("bucket", "http://127.0.0.1:9000"),
        ("bucket", "http://[::1]:9000"),
        ("Bucket", "https://s3.example.test"),
        ("my_bucket", "https://s3.example.test"),
    ] {
        let url = Url::parse(endpoint).unwrap();
        let settings = Settings {
            bucket,
            region: "us-east-1",
            endpoint: Some(&url),
            path_style: false,
        };
        assert_eq!(
            client::bucket_endpoint(&settings).unwrap_err(),
            Fault::Internal
        );
        let origin = client::origin(&Settings {
            path_style: true,
            ..settings
        })
        .unwrap();
        let connector = Connector::new(Script::default(), origin, Duration::from_secs(1));
        assert_eq!(
            client::build(&settings, None, &creds, &connector).unwrap_err(),
            Fault::Internal
        );
    }
}

fn percent_decode(path: &str) -> Vec<u8> {
    let bytes = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&path[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Keys passing 2.3's rules, of every class E37 names written as given.
fn keys() -> Vec<String> {
    let alphabet: Vec<char> = "aZ09-_.~%#+?&=' !*()\\@:;,$[]{}^`|<>\"é漢🎉/"
        .chars()
        .collect();
    let mut keys: Vec<String> = [
        "plain/key.png",
        "100% sure/#1 ~home.png",
        "a+b=c?d&e/f g.png",
        "unicodé/漢字/🎉.png",
        "quote'\"back\\slash`.png",
        "%2F/%41%",
        "..a/.b/c..",
        "brackets[]{}<>|^.png",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut state = 0x2545_f491_u64;
    while keys.len() < 300 {
        let mut key = String::new();
        for _ in 0..=state % 24 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            key.push(alphabet[(state >> 33) as usize % alphabet.len()]);
        }
        if unityrs_export::keys::check(key.as_bytes()).is_ok() {
            keys.push(key);
        }
    }
    keys
}

#[test]
fn every_key_the_rules_pass_round_trips_through_path_parse_and_the_request_uri() {
    let keys = keys();
    let h = Harness::new(keys.iter().map(|_| ok()));
    for (i, key) in keys.iter().enumerate() {
        assert!(
            unityrs_export::keys::check(key.as_bytes()).is_ok(),
            "{key:?}"
        );
        let path = Path::parse(key).unwrap();
        assert_eq!(path.as_ref(), key);
        // No URI the `url` crate would send differently: the put succeeds.
        put(&h, key).unwrap();
        let line = h.script.seen.line(i);
        let target = line.split(' ').nth(1).unwrap();
        assert!(target.is_ascii());
        assert_eq!(
            percent_decode(target),
            format!("/bucket/{key}").into_bytes(),
            "{key:?}"
        );
    }
}

fn raw(uri: &str, headers: &[(&str, &str)]) -> object_store::client::HttpRequest {
    let mut builder = Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(HttpRequestBody::empty()).unwrap()
}

/// Runs `requests` as one try's call through the connector's client.
fn one_try(
    requests: Vec<object_store::client::HttpRequest>,
    replies: Vec<Reply>,
) -> (Result<(), Fault>, usize) {
    let script = Script::new(replies);
    let url = endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let connector = Connector::new(
        script.clone(),
        client::origin(&settings).unwrap(),
        Duration::from_secs(1),
    );
    let client = connector.connect(&ClientOptions::new()).unwrap();
    let h = Harness::new([]);
    let requests = std::sync::Mutex::new(requests);
    let result = run(h.tries().run(&h.slot, Kind::ListPage, |_| {
        let batch: Vec<_> = std::mem::take(&mut *requests.lock().unwrap());
        let client = client.clone();
        async move {
            for request in batch {
                client.execute(request).await.map_err(|_| Ended::Error)?;
            }
            Ok(())
        }
    }));
    (
        result.map(|_| ()),
        script.sent.load(std::sync::atomic::Ordering::Acquire),
    )
}

#[test]
fn a_second_request_in_one_call_is_the_engines_fault() {
    let uri = "https://s3.example.test:9000/bucket?list-type=2";
    let (result, sent) = one_try(vec![raw(uri, &[]), raw(uri, &[])], vec![ok(), ok()]);
    assert_eq!(result.unwrap_err(), Fault::Internal);
    assert_eq!(sent, 1);
    let (result, sent) = one_try(vec![raw(uri, &[])], vec![ok()]);
    assert!(result.is_ok());
    assert_eq!(sent, 1);
}

#[test]
fn a_request_the_engine_would_not_send_as_signed_never_leaves() {
    for (uri, headers) in [
        // The `url` crate would encode the quote: other bytes than signed.
        ("https://s3.example.test:9000/bucket?prefix='x'", &[][..]),
        // Another origin: scheme, host or port.
        ("http://s3.example.test:9000/bucket", &[]),
        ("https://s3.other.test:9000/bucket", &[]),
        ("https://s3.example.test/bucket", &[]),
        // A Host that is not the URL's authority, or two.
        (
            "https://s3.example.test:9000/bucket",
            &[("host", "s3.example.test")],
        ),
        (
            "https://s3.example.test:9000/bucket",
            &[
                ("host", "s3.example.test:9000"),
                ("host", "s3.example.test:9000"),
            ],
        ),
        // A length that is not the body's, and headers the engine owns.
        (
            "https://s3.example.test:9000/bucket",
            &[("content-length", "4")],
        ),
        (
            "https://s3.example.test:9000/bucket",
            &[("user-agent", "other")],
        ),
        (
            "https://s3.example.test:9000/bucket",
            &[("accept", "text/xml")],
        ),
    ] {
        let (result, sent) = one_try(vec![raw(uri, headers)], vec![]);
        assert_eq!(result.unwrap_err(), Fault::Internal, "{uri} {headers:?}");
        assert_eq!(sent, 0);
    }
    let (result, sent) = one_try(
        vec![raw(
            "https://s3.example.test:9000/bucket?prefix=a%20b",
            &[("host", "s3.example.test:9000"), ("content-length", "0")],
        )],
        vec![ok()],
    );
    assert!(result.is_ok());
    assert_eq!(sent, 1);
}

#[test]
fn a_call_outside_a_try_sends_nothing() {
    let script = Script::new([ok()]);
    let url = endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let connector = Connector::new(
        script.clone(),
        client::origin(&settings).unwrap(),
        Duration::from_secs(1),
    );
    let client = connector.connect(&ClientOptions::new()).unwrap();
    let error = run(client.execute(raw("https://s3.example.test:9000/bucket", &[]))).unwrap_err();
    assert_eq!(error.to_string(), "HTTP error: store request failed");
    assert_eq!(script.sent.load(std::sync::atomic::Ordering::Acquire), 0);
}

/// An `object_store` error, as anything that rendered one would.
fn rendered_errors(error: &object_store::Error) {
    let text = format!("{error} {error:?}");
    assert!(!text.contains("sentinel"), "{text}");
}

#[test]
fn sentinel_credentials_render_nowhere() {
    const KEY: &str = "AKIDSENTINELKEY00001";
    const SECRET: &str = "sentinel/secret+value/never-rendered";
    const TOKEN: &str = "sentinel-session-token-never-rendered";
    let env = |name: &str| match name {
        "AWS_ACCESS_KEY_ID" => Some(KEY.into()),
        "AWS_SECRET_ACCESS_KEY" => Some(SECRET.into()),
        "AWS_SESSION_TOKEN" => Some(TOKEN.into()),
        _ => None,
    };
    let creds = unityrs_batch::store::credentials::read(env, None).unwrap();
    let mut rendered = vec![
        format!("{creds:?}"),
        format!("{}", creds.secret),
        format!("{}", creds.key_id),
    ];
    assert_eq!(
        rendered[0],
        "Credentials { key_id: <20 bytes>, secret: <36 bytes>, token: Some(<37 bytes>) }"
    );
    let provider = creds.into_provider();
    let url = endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let script = Script::new([
        Reply::error(403, "SignatureDoesNotMatch"),
        Reply::error(400, "ExpiredToken"),
        Reply::Refuse(unityrs_batch::http::Detail::Tls),
        Reply::status(
            503,
            "Retry-After: 1\r\n",
            b"<Error><Code>SlowDown</Code></Error>",
        ),
    ]);
    let connector = Connector::new(
        script.clone(),
        client::origin(&settings).unwrap(),
        Duration::from_secs(1),
    );
    let store = client::build(&settings, None, &provider, &connector).unwrap();
    let h = Harness::new([]);
    let path = Path::parse("k").unwrap();
    for _ in 0..4 {
        let done = run(h.tries().run(&h.slot, Kind::Put, |_| {
            let (store, path) = (&store, &path);
            async move {
                let error = store
                    .put(path, PutPayload::from_static(b"x"))
                    .await
                    .unwrap_err();
                rendered_errors(&error);
                Err::<(), _>(Ended::Error)
            }
        }));
        let done = done.unwrap();
        rendered.push(format!("{done:?}"));
    }
    let error = run(store.put(&path, PutPayload::from_static(b"x"))).unwrap_err();
    rendered.push(format!("{error} {error:?}"));
    for text in &rendered {
        for secret in [SECRET, TOKEN] {
            assert!(!text.contains(secret), "{text}");
        }
    }
    // The secret only keys the signature: no request carries it.
    for i in 0..script.seen.count() {
        assert!(!script.seen.request(i).contains(SECRET));
    }
    let _ = Arc::clone(&h.slot);
}
