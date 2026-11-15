//! Counted peer backing stays alive outside the measurement through parsing.
//! Reading a reply never frees a pre-window block from the engine's count.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use unityrs_batch::{
    http::{
        self, Payload, Response,
        permit::{OperationToken, RequestPermit},
    },
    store::connector::Transport,
};
use url::Url;
#[derive(Clone)]
pub struct Replay(pub Arc<[u8]>);
pub struct Io {
    reply: Arc<[u8]>,
    at: usize,
    head: usize,
    reader: Option<Waker>,
}
impl Transport for Replay {
    type Io = Io;
    async fn send(
        &self,
        permit: &mut RequestPermit,
        _: &Url,
        request: hyper::Request<Payload>,
        _: Duration,
        token: OperationToken,
        connected: &AtomicBool,
    ) -> Result<Response<Io>, http::Error> {
        drop(token);
        connected.store(true, Ordering::Release);
        http::exchange(
            Io {
                reply: self.0.clone(),
                at: 0,
                head: 0,
                reader: None,
            },
            permit,
            request,
        )
        .await
    }
}
impl AsyncRead for Io {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.head != 4 {
            self.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = (self.reply.len() - self.at).min(out.remaining());
        out.put_slice(&self.reply[self.at..self.at + n]);
        self.at += n;
        Poll::Ready(Ok(()))
    }
}
impl AsyncWrite for Io {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        for &byte in bytes {
            if self.head < 4 {
                if b"\r\n\r\n"[self.head] == byte {
                    self.head += 1;
                } else {
                    self.head = usize::from(byte == b'\r');
                }
            }
        }
        if let Some(w) = self.reader.take() {
            w.wake();
        }
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
