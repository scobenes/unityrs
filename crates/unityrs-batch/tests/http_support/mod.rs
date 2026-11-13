//! Deterministic in-memory byte stream. Every read boundary is explicit.
#![allow(dead_code)]
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use unityrs_batch::http::Payload;
use unityrs_testkit::guard::DEADLOCK_GUARD;

pub fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(DEADLOCK_GUARD, future)
                .await
                .expect("deadlock guard")
        })
}

pub fn get() -> hyper::Request<Payload> {
    hyper::Request::builder()
        .uri("/")
        .header("host", "localhost")
        .body(Payload::default())
        .unwrap()
}

pub struct MemoryIo {
    input: std::sync::Arc<[u8]>,
    offset: usize,
    fragment: usize,
    pub writes: Vec<u8>,
    read_error: Option<usize>,
    write_error: bool,
    reader: Option<Waker>,
}
impl MemoryIo {
    pub fn backing(&self) -> std::sync::Arc<[u8]> {
        self.input.clone()
    }
    pub fn failing_read(mut self, offset: usize) -> Self {
        self.read_error = Some(offset);
        self
    }
    pub fn failing_write(mut self) -> Self {
        self.write_error = true;
        self
    }
    pub fn new(input: Vec<u8>, fragment: usize) -> Self {
        Self {
            input: input.into(),
            offset: 0,
            fragment,
            writes: Vec::new(),
            read_error: None,
            write_error: false,
            reader: None,
        }
    }
}
impl AsyncRead for MemoryIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // A server cannot send this response before receiving the request.
        // Hyper probes idle connections before polling its request queue.
        if self.writes.is_empty() {
            self.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        if self.read_error.is_some_and(|offset| self.offset >= offset) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        let available = self
            .read_error
            .unwrap_or(self.input.len())
            .min(self.input.len());
        let count = (available - self.offset)
            .min(buf.remaining())
            .min(self.fragment);
        let end = self.offset + count;
        buf.put_slice(&self.input[self.offset..end]);
        self.offset = end;
        Poll::Ready(Ok(()))
    }
}
impl AsyncWrite for MemoryIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_error {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        self.writes.extend_from_slice(buf);
        if let Some(reader) = self.reader.take() {
            reader.wake();
        }
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub fn large_head(size: usize, complete: bool) -> Vec<u8> {
    let mut bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nx: ".to_vec();
    let suffix = if complete {
        b"\r\n\r\n".as_slice()
    } else {
        b""
    };
    bytes.resize(size - suffix.len(), b'a');
    bytes.extend_from_slice(suffix);
    bytes
}

#[cfg(feature = "test-hooks")]
pub mod stress;
#[cfg(feature = "test-hooks")]
pub mod tls;

/// Maximum 1xx and final heads, a large DATA backing, and a near-cap trailer.
pub fn combined_response() -> Vec<u8> {
    fn padded(prefix: &[u8]) -> Vec<u8> {
        let mut bytes = prefix.to_vec();
        for index in 0..98 {
            bytes.extend_from_slice(format!("x-head-{index}: a\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"x-last: ");
        bytes.resize(131_068, b'a');
        bytes.extend_from_slice(b"\r\n\r\n");
        bytes
    }
    let mut bytes = padded(b"HTTP/1.1 103 Early Hints\r\n");
    bytes.extend_from_slice(&padded(
        b"HTTP/1.1 200 Custom Reason\r\nTransfer-Encoding: chunked\r\n",
    ));
    bytes.extend_from_slice(b"20000\r\n");
    bytes.extend(std::iter::repeat_n(b'x', 131_072));
    bytes.extend_from_slice(b"\r\n0\r\n");
    let trailer_start = bytes.len();
    for index in 0..99 {
        bytes.extend_from_slice(format!("x-trailer-{index}: a\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"x-last: ");
    bytes.resize(trailer_start + 131_067, b'y');
    bytes.extend_from_slice(b"\r\n\r\n");
    bytes
}
