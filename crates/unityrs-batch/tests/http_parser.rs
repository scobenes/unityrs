//! Frozen raw HTTP fixtures against the exact hyper pin; no sockets or DNS.
mod http_support;
use http_support::{MemoryIo, run};
use unityrs_batch::http::{self, Detail, Error, Payload, permit::PermitPool};

fn head(bytes: Vec<u8>, fragment: usize) -> Result<http::Response<MemoryIo>, Error> {
    let pool = PermitPool::new(1).unwrap();
    let permit = pool.acquire().unwrap();
    run(http::exchange(
        MemoryIo::new(bytes, fragment),
        &permit,
        http_support::get(),
    ))
}

#[test]
fn complete_head_at_the_cap_and_unfinished_head_at_the_cap_differ() {
    for size in [131_071, 131_072] {
        let response = head(http_support::large_head(size, true), usize::MAX).unwrap();
        assert_eq!(response.status(), 200);
    }
    assert_eq!(
        head(http_support::large_head(131_071, false), 1_024)
            .err()
            .unwrap()
            .detail(),
        Some(Detail::NoResponse)
    );
    for fragment in [1_024, usize::MAX] {
        let error = head(http_support::large_head(131_073, true), fragment)
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::HeadTooLarge));
        let error = head(http_support::large_head(131_072, false), fragment)
            .err()
            .unwrap();
        assert_eq!(error.detail(), Some(Detail::HeadTooLarge));
    }
}

#[test]
fn typed_head_classification_covers_fields_names_lengths_and_malformed_input() {
    let mut fields = b"HTTP/1.1 200 OK\r\n".to_vec();
    for _ in 0..100 {
        fields.extend_from_slice(b"x: y\r\n");
    }
    fields.extend_from_slice(b"\r\n");
    assert_eq!(head(fields.clone(), 1_024).unwrap().status(), 200);
    fields.splice(17..17, b"x: y\r\n".iter().copied());
    assert_eq!(
        head(fields, 1_024).err().unwrap().detail(),
        Some(Detail::HeadTooLarge)
    );
    for (length, expected) in [
        ("18446744073709551613", None),
        ("18446744073709551614", Some(Detail::HeadTooLarge)),
        ("18446744073709551615", Some(Detail::HeadTooLarge)),
        ("18446744073709551616", Some(Detail::NoResponse)),
        ("-1", Some(Detail::NoResponse)),
    ] {
        let bytes = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n").into_bytes();
        let result = head(bytes, 1_024);
        assert_eq!(result.err().and_then(Error::detail), expected, "{length}");
    }
    for bytes in [
        b"garbage\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\ninvalid\r\n\r\n",
        b"HTTP/1.1 200",
    ] {
        assert_eq!(
            head(bytes.to_vec(), 1).err().unwrap().detail(),
            Some(Detail::NoResponse)
        );
    }
    for size in [65_535, 65_536] {
        let mut bytes = b"HTTP/1.1 200 OK\r\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', size));
        bytes.extend_from_slice(b": x\r\n\r\n");
        let expected = (size == 65_536).then_some(Detail::HeadTooLarge);
        assert_eq!(head(bytes, 1_024).err().and_then(Error::detail), expected);
    }
}

#[test]
fn informational_heads_obey_the_same_cap_and_101_is_final() {
    let mut bytes = http_support::large_head(131_073, true);
    bytes.splice(9..12, b"103".iter().copied());
    assert_eq!(
        head(bytes, usize::MAX).err().unwrap().detail(),
        Some(Detail::HeadTooLarge)
    );
    let response = head(b"HTTP/1.1 103 Early Hints\r\n\r\nHTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: test\r\n\r\n".to_vec(), 1).unwrap();
    assert_eq!(response.status(), 101);
}

#[test]
fn body_failures_are_truncated_and_an_error_status_does_not_read_its_body() {
    for body in [
        b"Content-Length: 2\r\n\r\nx".as_slice(),
        b"Transfer-Encoding: chunked\r\n\r\n1\r\nx\r\n",
        b"Transfer-Encoding: chunked\r\n\r\nx\r\n",
    ] {
        let mut bytes = b"HTTP/1.1 200 OK\r\n".to_vec();
        bytes.extend_from_slice(body);
        let mut response = head(bytes, 1_024).unwrap();
        let error = run(async {
            while response.data().await?.is_some() {}
            Ok::<(), Error>(())
        })
        .unwrap_err();
        assert_eq!(error.detail(), Some(Detail::Truncated));
    }
    let response = head(
        b"HTTP/1.1 503 Unavailable\r\nTransfer-Encoding: chunked\r\n\r\ninvalid".to_vec(),
        usize::MAX,
    )
    .unwrap();
    assert_eq!(response.status(), 503);
}

#[test]
fn request_body_chunks_have_an_exact_hint_and_never_exceed_64_kib() {
    use hyper::body::Body;
    let mut body = Payload::new(vec![7; 196_609].into());
    assert_eq!(body.size_hint().exact(), Some(196_609));
    run(async {
        use http_body_util::BodyExt;
        let mut total = 0;
        while let Some(frame) = body.frame().await {
            let data = frame.unwrap().into_data().unwrap();
            assert!(data.len() <= 65_536);
            total += data.len();
        }
        assert_eq!(total, 196_609);
        assert_eq!(body.size_hint().exact(), Some(0));
    });
}

#[test]
fn trailers_refuse_equality_and_more_than_100_fields() {
    for size in [131_071, 131_072, 131_073] {
        let mut bytes = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nx: ".to_vec();
        let prefix = bytes.len() - 3;
        bytes.resize(prefix + size - 4, b'a');
        bytes.extend_from_slice(b"\r\n\r\n");
        let mut response = head(bytes, usize::MAX).unwrap();
        let result = run(response.data());
        if size < 131_072 {
            assert!(result.unwrap().is_none());
        } else {
            assert_eq!(result.err().unwrap().detail(), Some(Detail::Truncated));
        }
    }
    for fields in [100, 101] {
        let mut bytes = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n".to_vec();
        for _ in 0..fields {
            bytes.extend_from_slice(b"x: y\r\n");
        }
        bytes.extend_from_slice(b"\r\n");
        let mut response = head(bytes, 1_024).unwrap();
        let result = run(response.data());
        if fields == 100 {
            assert!(result.unwrap().is_none());
        } else {
            assert_eq!(result.err().unwrap().detail(), Some(Detail::Truncated));
        }
    }
}

#[test]
fn copied_pieces_are_bounded_while_the_original_backing_remains_accounted() {
    let mut bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 131073\r\n\r\n".to_vec();
    bytes.resize(bytes.len() + 131_073, b'a');
    let mut response = head(bytes, usize::MAX).unwrap();
    run(async {
        let mut total = 0;
        while let Some(data) = response.data().await.unwrap() {
            assert!(data.as_slice().len() <= 65_536);
            total += data.as_slice().len();
        }
        assert_eq!(total, 131_073);
    });
}

#[test]
fn send_failure_reset_before_head_and_reset_mid_body_use_the_failing_layer() {
    let pool = PermitPool::new(1).unwrap();
    let permit = pool.acquire().unwrap();
    let io = MemoryIo::new(Vec::new(), 1).failing_write();
    let error = run(http::exchange(io, &permit, http_support::get()))
        .err()
        .unwrap();
    assert_eq!(error.detail(), Some(Detail::NoResponse));
    let io = MemoryIo::new(b"HTTP/1.1 200 OK\r\n".to_vec(), 1).failing_read(5);
    let error = run(http::exchange(io, &permit, http_support::get()))
        .err()
        .unwrap();
    assert_eq!(error.detail(), Some(Detail::NoResponse));
    let bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nLocation: /?new-secret=hidden\r\n\r\nx"
        .to_vec();
    let end = bytes.len();
    let io = MemoryIo::new(bytes, 1).failing_read(end);
    let mut response = run(http::exchange(io, &permit, http_support::get()))
        .ok()
        .unwrap();
    let error = run(async {
        while response.data().await?.is_some() {}
        Ok::<_, Error>(())
    })
    .unwrap_err();
    assert_eq!(error.detail(), Some(Detail::Truncated));
    assert_eq!(
        format!("{error:?}"),
        "Transport(Failure { detail: Truncated, errno: None })"
    );
}

#[test]
fn a_terminal_body_error_is_returned_on_every_later_read() {
    let mut response = head(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx".to_vec(),
        1_024,
    )
    .unwrap();
    run(async {
        let error = loop {
            match response.data().await {
                Ok(Some(_)) => (),
                Ok(None) => panic!("truncated response reported success"),
                Err(error) => break error,
            }
        };
        assert_eq!(error.detail(), Some(Detail::Truncated));
        for _ in 0..3 {
            assert_eq!(response.data().await.err(), Some(error));
        }
    });
}
